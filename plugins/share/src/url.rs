//! The URL validator `Share::url` runs before touching a platform API.
//!
//! Adapted from `plugins/url-launcher/src/url.rs` (same rule table, same
//! validator body; only the error type name differs).
//!
//! Deliberately self-contained — no dependency on this crate's own
//! [`ShareError`](crate::ShareError) variants beyond the single
//! [`InvalidUrl`](crate::ShareError::InvalidUrl) case, and no
//! third-party URL-parsing crate — so it can be copied between plugins
//! without pulling in any platform dependency edge.
//!
//! # Rule table
//!
//! A URL is rejected unless **all** of the following hold:
//!
//! - Non-empty and at most [`MAX_LEN`] bytes.
//! - Every byte is ASCII, with no control byte (`0x00..=0x1F`, `0x7F`) and no
//!   literal whitespace, including the ASCII space (`0x20`) — a
//!   percent-encoded space (`%20`) is fine. This also rejects any non-ASCII
//!   byte (a raw UTF-8 IRI) outright: `url::validate` accepts only
//!   pre-encoded ASCII URLs.
//! - The **scheme** — the bytes before the first `:` — is `http` or `https`,
//!   case-insensitively, and is immediately followed by `//`. This closes
//!   the `javascript:`/`intent:`/`tel:`/`file:` deep-link/script-injection
//!   classes: none of those schemes ever reaches a backend.
//! - The **authority** — from there up to the first `/`, `?` or `#` (or the
//!   end of the string) — is non-empty and contains no `@`. Userinfo
//!   (`user:pw@host`) is rejected outright rather than silently dropped or
//!   misparsed — a classic phishing vector (`https://trusted.example@evil.test/`
//!   reads as "trusted.example" to a human, "evil.test" to a URL parser).
//! - Every byte in the URL — scheme, authority, path, query, fragment alike —
//!   is either RFC 3986 `unreserved` (`A-Za-z0-9-._~`), `reserved`
//!   (`:/?#[]@!$&'()*+,;=`), or `%` followed by exactly two hex digits.
//!
//! This is not a full RFC 3986 parser (no percent-decoding, no relative
//! resolution, no IPv6-literal-in-brackets special-casing beyond `[`/`]`
//! already being reserved characters) — it is a narrow allow-list sufficient
//! to keep an advisory, fire-and-forget "open this in the browser" call from
//! ever reaching a non-http(s) handler or leaking credentials, nothing more.

use crate::ShareError;

/// Maximum allowed URL length in bytes — a generous ceiling well above any
/// realistic browser address-bar length, chosen to bound validation cost and
/// reject pathological input outright rather than pursue a "correct" browser
/// limit (browsers themselves disagree on one).
const MAX_LEN: usize = 8192;

/// RFC 3986 `unreserved` characters: `A-Za-z0-9-._~`.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// RFC 3986 `reserved` characters (`gen-delims` + `sub-delims`):
/// `:/?#[]@!$&'()*+,;=`.
fn is_reserved(byte: u8) -> bool {
    matches!(
        byte,
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
    )
}

/// Validate `url` against the module doc's rule table. `Ok(())` means `url`
/// is safe to hand to any of this crate's backends unmodified.
pub(crate) fn validate(url: &str) -> Result<(), ShareError> {
    if url.is_empty() || url.len() > MAX_LEN {
        return Err(ShareError::InvalidUrl);
    }

    let bytes = url.as_bytes();
    if !bytes.is_ascii() {
        return Err(ShareError::InvalidUrl);
    }
    if bytes
        .iter()
        .any(|&byte| byte < 0x20 || byte == 0x7F || byte == b' ')
    {
        return Err(ShareError::InvalidUrl);
    }

    let Some(colon) = bytes.iter().position(|&byte| byte == b':') else {
        return Err(ShareError::InvalidUrl);
    };
    let scheme = &url[..colon];
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(ShareError::InvalidUrl);
    }
    if bytes.get(colon + 1) != Some(&b'/') || bytes.get(colon + 2) != Some(&b'/') {
        return Err(ShareError::InvalidUrl);
    }

    let rest = &bytes[colon + 3..];
    let authority_end = rest
        .iter()
        .position(|&byte| byte == b'/' || byte == b'?' || byte == b'#')
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains(&b'@') {
        return Err(ShareError::InvalidUrl);
    }

    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let hex = (bytes.get(index + 1).copied(), bytes.get(index + 2).copied());
            match hex {
                (Some(h1), Some(h2)) if h1.is_ascii_hexdigit() && h2.is_ascii_hexdigit() => {
                    index += 3;
                    continue;
                }
                _ => return Err(ShareError::InvalidUrl),
            }
        }
        if !is_unreserved(byte) && !is_reserved(byte) {
            return Err(ShareError::InvalidUrl);
        }
        index += 1;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_schemes() {
        for url in [
            "javascript:alert(1)",
            "intent://x#Intent;end",
            "tel:+1",
            "file:///etc/passwd",
        ] {
            assert!(validate(url).is_err(), "{url} should be rejected");
        }
    }

    #[test]
    fn rejects_malformed_scheme_or_authority() {
        for url in [
            "HTTP:/relative",
            "/path",
            "example.com",
            "",
            "https://user:pw@host/",
            "http://",
            "https:///path",
        ] {
            assert!(validate(url).is_err(), "{url} should be rejected");
        }
    }

    #[test]
    fn rejects_non_ascii_and_bad_bytes() {
        for url in [
            "https://exämple.com/",
            "https://host/ a",
            "https://host/%zz",
            "https://host/%2",
        ] {
            assert!(validate(url).is_err(), "{url} should be rejected");
        }
    }

    #[test]
    fn rejects_oversized_url() {
        let mut url = "https://host/".to_string();
        url.push_str(&"a".repeat(8193 - url.len()));
        assert_eq!(url.len(), 8193);
        assert!(validate(&url).is_err());
    }

    /// The doc says "at most [`MAX_LEN`] bytes" — pins the accept side of
    /// that boundary, not just the 8193-byte rejection above.
    #[test]
    fn accepts_url_at_exact_max_len_boundary() {
        let mut url = "https://host/".to_string();
        url.push_str(&"a".repeat(MAX_LEN - url.len()));
        assert_eq!(url.len(), MAX_LEN);
        assert!(validate(&url).is_ok());
    }

    #[test]
    fn accepts_well_formed_http_and_https_urls() {
        for url in [
            "https://host/path?q=a%20b#frag",
            "HTTPS://HOST",
            "http://127.0.0.1:8080/x",
            "https://xn--bcher-kva.example/",
            "https://host:443/a/b?c=d&e=f%3D",
        ] {
            assert!(validate(url).is_ok(), "{url} should be accepted");
        }
    }
}
