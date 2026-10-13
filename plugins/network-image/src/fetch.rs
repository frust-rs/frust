//! The fetch seam: a trait any app can implement with its own HTTP client,
//! plus the request/response/error types and the URL gate every backend
//! runs first.

use std::future::Future;
use std::pin::Pin;

/// Maximum accepted URL length in bytes.
const MAX_URL_LEN: usize = 8192;

/// One image fetch.
#[derive(Clone, Debug)]
pub struct FetchRequest {
    /// Absolute `http`/`https` URL (run through [`validate_url`] by backends).
    pub url: String,
    /// Extra request headers, e.g. `Authorization`.
    pub headers: Vec<(String, String)>,
    /// Abort with [`FetchError::TooLarge`] once the body exceeds this many bytes.
    pub max_bytes: usize,
}

/// A completed fetch.
#[derive(Clone, Debug)]
pub struct FetchedBytes {
    /// The response body.
    pub bytes: Vec<u8>,
    /// The `Content-Type` header, when present and valid UTF-8.
    pub content_type: Option<String>,
}

/// Why a fetch failed. `Display` never echoes the URL or a header value.
#[non_exhaustive]
#[derive(thiserror::Error, Debug, Clone)]
pub enum FetchError {
    /// The URL failed [`validate_url`].
    #[error("invalid image URL")]
    InvalidUrl,
    /// The server answered with a non-2xx status.
    #[error("server answered with status {0}")]
    Status(u16),
    /// The body exceeded the request's `max_bytes`.
    #[error("response body exceeds the {limit}-byte limit")]
    TooLarge {
        /// The limit that was exceeded.
        limit: usize,
    },
    /// The request timed out.
    #[error("request timed out")]
    Timeout,
    /// Any other transport failure (connect, TLS, protocol), as an error-kind string.
    #[error("network error: {0}")]
    Network(String),
    /// The fetch was cancelled before it completed.
    #[error("fetch cancelled")]
    Cancelled,
}

/// The boxed future an [`ImageFetcher`] returns.
pub type FetchFuture =
    Pin<Box<dyn Future<Output = Result<FetchedBytes, FetchError>> + Send + 'static>>;

/// A byte source for image URLs. Implement it to route fetches through an
/// app-owned HTTP client; the default is `ReqwestFetcher`.
pub trait ImageFetcher: Send + Sync + 'static {
    /// Fetch `request`. Implementations call [`validate_url`] first.
    fn fetch(&self, request: FetchRequest) -> FetchFuture;
}

/// Accept only absolute `http`/`https` URLs of at most 8192 bytes, pure
/// ASCII without control bytes or spaces, with a non-empty authority and no
/// userinfo.
///
/// Mirrors the rule table of `plugins/url-launcher/src/url.rs` (same scheme,
/// userinfo, length and character allow-list rules).
pub fn validate_url(url: &str) -> Result<(), FetchError> {
    let bad = Err(FetchError::InvalidUrl);
    if url.is_empty() || url.len() > MAX_URL_LEN {
        return bad;
    }
    let bytes = url.as_bytes();
    if !bytes.is_ascii() || bytes.iter().any(|&b| b < 0x20 || b == 0x7F || b == b' ') {
        return bad;
    }
    let Some(colon) = bytes.iter().position(|&b| b == b':') else {
        return bad;
    };
    let scheme = &url[..colon];
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return bad;
    }
    if !url[colon + 1..].starts_with("//") {
        return bad;
    }
    let rest = &bytes[colon + 3..];
    let end = rest
        .iter()
        .position(|&b| matches!(b, b'/' | b'?' | b'#'))
        .unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() || authority.contains(&b'@') {
        return bad;
    }
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            match (bytes.get(i + 1), bytes.get(i + 2)) {
                (Some(h1), Some(h2)) if h1.is_ascii_hexdigit() && h2.is_ascii_hexdigit() => {
                    i += 3;
                    continue;
                }
                _ => return bad,
            }
        }
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~');
        let reserved = matches!(
            b,
            b':' | b'/'
                | b'?'
                | b'#'
                | b'['
                | b']'
                | b'@'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
        );
        if !unreserved && !reserved {
            return bad;
        }
        i += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https() {
        assert!(validate_url("https://example.com/a.png?x=1%20y").is_ok());
        assert!(validate_url("HTTP://example.com").is_ok());
    }

    #[test]
    fn rejects_bad_urls() {
        for url in [
            "",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://user:pw@evil.test/",
            "https:///path",
            "https://a b/",
            "https://example.com/%zz",
            "https://exämple.com/",
        ] {
            assert!(
                matches!(validate_url(url), Err(FetchError::InvalidUrl)),
                "{url}"
            );
        }
        let long = format!("https://e.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(validate_url(&long).is_err());
    }

    #[test]
    fn display_does_not_echo_input() {
        assert!(
            !FetchError::Network("kind".into())
                .to_string()
                .contains("http")
        );
    }
}
