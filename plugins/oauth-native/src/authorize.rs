//! The authorization request URL (RFC 6749 §4.1.1 with RFC 7636 PKCE and
//! RFC 8707 resource indicators).

use crate::encode::{decode_component, encode_component};
use crate::pkce::{PkceChallenge, State};

/// Parameter names [`AuthorizationRequest::build`] writes itself; an
/// endpoint whose existing query already carries one is refused rather than
/// sending the parameter twice.
const OWNED_PARAMETERS: [&str; 8] = [
    "response_type",
    "client_id",
    "redirect_uri",
    "state",
    "code_challenge",
    "code_challenge_method",
    "scope",
    "resource",
];

/// The authorization URL could not be built. Messages never echo a URL.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthorizeError {
    /// The authorization endpoint is not an `https://` URL.
    #[error("the authorization endpoint must be an https:// URL")]
    InsecureEndpoint,
    /// The authorization endpoint has no host, contains a fragment,
    /// whitespace or a control character, or has an undecodable query.
    #[error("the authorization endpoint is not a valid URL")]
    InvalidEndpoint,
    /// The endpoint's existing query already carries a parameter this
    /// request sets.
    #[error("the authorization endpoint already carries the `{0}` parameter")]
    DuplicateParameter(&'static str),
    /// `client_id` is empty.
    #[error("client_id is empty")]
    EmptyClientId,
    /// `redirect_uri` is empty.
    #[error("redirect_uri is empty")]
    EmptyRedirectUri,
    /// A `resource` is not an absolute URI, or contains a fragment
    /// (RFC 8707 §2). The payload is its index in `resources`.
    #[error("resource #{0} must be an absolute URI without a fragment")]
    InvalidResource(usize),
}

/// The fixed inputs of an authorization-code request; [`build`](Self::build)
/// combines them with a fresh [`State`] and [`PkceChallenge`] into the URL
/// the in-app browser session opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationRequest {
    /// The authorization server's `authorization_endpoint` (must be
    /// `https://`). It may already carry a query of its own.
    pub authorization_endpoint: String,
    /// The registered client identifier.
    pub client_id: String,
    /// The redirect URI registered for this client — a private-use scheme
    /// (`io.example.app:/oauth/callback`) or a loopback
    /// (`http://127.0.0.1:<port>/…`) URI (RFC 8252 §7).
    pub redirect_uri: String,
    /// Space-separated scopes, if any.
    pub scope: Option<String>,
    /// RFC 8707 resource indicators; one `resource` parameter each.
    pub resources: Vec<String>,
}

impl AuthorizationRequest {
    /// Builds the authorization URL. Parameters, in order:
    /// `response_type=code`, `client_id`, `redirect_uri`, `scope` (when
    /// set), one `resource` per entry, `state`, `code_challenge`,
    /// `code_challenge_method=S256`. Values are percent-encoded per RFC
    /// 3986 (a space is `%20`).
    pub fn build(
        &self,
        state: &State,
        challenge: &PkceChallenge,
    ) -> Result<String, AuthorizeError> {
        let endpoint = self.authorization_endpoint.as_str();
        validate_endpoint(endpoint)?;
        if self.client_id.is_empty() {
            return Err(AuthorizeError::EmptyClientId);
        }
        if self.redirect_uri.is_empty() {
            return Err(AuthorizeError::EmptyRedirectUri);
        }
        for (index, resource) in self.resources.iter().enumerate() {
            if !is_absolute_uri(resource) || resource.contains('#') {
                return Err(AuthorizeError::InvalidResource(index));
            }
        }

        let mut params: Vec<(&str, &str)> = vec![
            ("response_type", "code"),
            ("client_id", &self.client_id),
            ("redirect_uri", &self.redirect_uri),
        ];
        if let Some(scope) = &self.scope {
            params.push(("scope", scope));
        }
        for resource in &self.resources {
            params.push(("resource", resource));
        }
        params.push(("state", state.as_str()));
        params.push(("code_challenge", challenge.as_str()));
        params.push(("code_challenge_method", challenge.method()));

        let mut url = String::from(endpoint);
        match endpoint.split_once('?') {
            None => url.push('?'),
            Some((_, query)) if query.is_empty() || query.ends_with('&') => {}
            Some(_) => url.push('&'),
        }
        let query: Vec<String> = params
            .into_iter()
            .map(|(name, value)| format!("{name}={}", encode_component(value)))
            .collect();
        url.push_str(&query.join("&"));
        Ok(url)
    }
}

fn validate_endpoint(endpoint: &str) -> Result<(), AuthorizeError> {
    const HTTPS: &str = "https://";
    let has_https = endpoint
        .get(..HTTPS.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(HTTPS));
    if !has_https {
        return Err(AuthorizeError::InsecureEndpoint);
    }
    if endpoint.contains('#')
        || endpoint
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(AuthorizeError::InvalidEndpoint);
    }
    let rest = &endpoint[HTTPS.len()..];
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    if authority_end == 0 {
        return Err(AuthorizeError::InvalidEndpoint);
    }
    if let Some((_, query)) = endpoint.split_once('?') {
        for segment in query.split('&').filter(|s| !s.is_empty()) {
            let raw_name = segment.split_once('=').map_or(segment, |(name, _)| name);
            let name = decode_component(raw_name).map_err(|_| AuthorizeError::InvalidEndpoint)?;
            if let Some(owned) = OWNED_PARAMETERS.iter().find(|owned| **owned == name) {
                return Err(AuthorizeError::DuplicateParameter(owned));
            }
        }
    }
    Ok(())
}

/// Whether `uri` starts with an RFC 3986 scheme and `:` —
/// `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`.
fn is_absolute_uri(uri: &str) -> bool {
    let Some((scheme, _)) = uri.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkce::PkceVerifier;

    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    fn fixtures() -> (State, PkceChallenge) {
        let state = State::from_string("st4te-value_~.st4te-value".to_string()).unwrap();
        let challenge = PkceVerifier::from_string(VERIFIER.to_string())
            .unwrap()
            .challenge();
        (state, challenge)
    }

    fn request(endpoint: &str) -> AuthorizationRequest {
        AuthorizationRequest {
            authorization_endpoint: endpoint.to_string(),
            client_id: "client 1".to_string(),
            redirect_uri: "com.example.app:/oauth/callback".to_string(),
            scope: Some("openid profile café".to_string()),
            resources: Vec::new(),
        }
    }

    #[test]
    fn builds_the_exact_url() {
        let (state, challenge) = fixtures();
        let url = request("https://as.example/authorize")
            .build(&state, &challenge)
            .unwrap();
        assert_eq!(
            url,
            format!(
                "https://as.example/authorize?response_type=code&client_id=client%201\
                 &redirect_uri=com.example.app%3A%2Foauth%2Fcallback\
                 &scope=openid%20profile%20caf%C3%A9&state=st4te-value_~.st4te-value\
                 &code_challenge={CHALLENGE}&code_challenge_method=S256"
            )
        );
    }

    #[test]
    fn omits_scope_when_none() {
        let (state, challenge) = fixtures();
        let mut req = request("https://as.example/authorize");
        req.scope = None;
        let url = req.build(&state, &challenge).unwrap();
        assert!(!url.contains("scope="));
        assert!(url.contains("redirect_uri=com.example.app%3A%2Foauth%2Fcallback&state="));
    }

    #[test]
    fn appends_to_an_existing_query_with_ampersand() {
        let (state, challenge) = fixtures();
        let url = request("https://as.example/authorize?tenant=acme")
            .build(&state, &challenge)
            .unwrap();
        assert!(url.starts_with("https://as.example/authorize?tenant=acme&response_type=code&"));
        let url = request("https://as.example/authorize?")
            .build(&state, &challenge)
            .unwrap();
        assert!(url.starts_with("https://as.example/authorize?response_type=code&"));
    }

    #[test]
    fn refuses_a_parameter_the_endpoint_already_carries() {
        let (state, challenge) = fixtures();
        for name in OWNED_PARAMETERS {
            let endpoint = format!("https://as.example/authorize?x=1&{name}=evil");
            assert_eq!(
                request(&endpoint).build(&state, &challenge),
                Err(AuthorizeError::DuplicateParameter(name))
            );
        }
        // An encoded name is caught too.
        assert_eq!(
            request("https://as.example/authorize?st%61te=x").build(&state, &challenge),
            Err(AuthorizeError::DuplicateParameter("state"))
        );
    }

    #[test]
    fn refuses_non_https_and_malformed_endpoints() {
        let (state, challenge) = fixtures();
        for endpoint in [
            "http://as.example/authorize",
            "as.example/authorize",
            "ftp://x",
            "",
        ] {
            assert_eq!(
                request(endpoint).build(&state, &challenge),
                Err(AuthorizeError::InsecureEndpoint),
                "{endpoint}"
            );
        }
        for endpoint in [
            "https://",
            "https:///authorize",
            "https://as.example/authorize#frag",
            "https://as.example/auth orize",
            "https://as.example/authorize\n",
            "https://as.example/authorize?%zz=1",
        ] {
            assert_eq!(
                request(endpoint).build(&state, &challenge),
                Err(AuthorizeError::InvalidEndpoint),
                "{endpoint:?}"
            );
        }
        // The scheme compares case-insensitively.
        assert!(
            request("HTTPS://as.example/authorize")
                .build(&state, &challenge)
                .is_ok()
        );
    }

    #[test]
    fn refuses_empty_client_and_redirect() {
        let (state, challenge) = fixtures();
        let mut req = request("https://as.example/authorize");
        req.client_id.clear();
        assert_eq!(
            req.build(&state, &challenge),
            Err(AuthorizeError::EmptyClientId)
        );
        let mut req = request("https://as.example/authorize");
        req.redirect_uri.clear();
        assert_eq!(
            req.build(&state, &challenge),
            Err(AuthorizeError::EmptyRedirectUri)
        );
    }

    #[test]
    fn writes_one_resource_parameter_per_entry() {
        let (state, challenge) = fixtures();
        let mut req = request("https://as.example/authorize");
        req.scope = None;
        req.resources = vec![
            "https://api.example/".to_string(),
            "urn:example:resource".to_string(),
        ];
        let url = req.build(&state, &challenge).unwrap();
        assert!(url.contains(
            "&redirect_uri=com.example.app%3A%2Foauth%2Fcallback\
             &resource=https%3A%2F%2Fapi.example%2F&resource=urn%3Aexample%3Aresource&state="
        ));
    }

    #[test]
    fn refuses_relative_or_fragment_resources() {
        let (state, challenge) = fixtures();
        for (bad, index) in [
            ("/relative", 1),
            ("https://api.example/#x", 1),
            ("1http:x", 1),
        ] {
            let mut req = request("https://as.example/authorize");
            req.resources = vec!["https://ok.example".to_string(), bad.to_string()];
            assert_eq!(
                req.build(&state, &challenge),
                Err(AuthorizeError::InvalidResource(index)),
                "{bad}"
            );
        }
    }
}
