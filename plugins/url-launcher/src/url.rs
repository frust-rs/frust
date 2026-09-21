//! The one URL validator every backend calls before touching a platform API
//! (`UrlLauncher::open_external`'s first step).
//!
//! Deliberately self-contained — no dependency on this crate's own
//! [`UrlLauncherError`](crate::UrlLauncherError) variants beyond the single
//! [`InvalidUrl`](crate::UrlLauncherError::InvalidUrl) case, and no
//! third-party URL-parsing crate — so `plugins/auth-session` can copy this
//! file byte-for-byte later
//! without pulling this crate's `frust-plugin`/JNI/objc2 dependency edge
//! along with it (see this crate's originating task).
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

use crate::UrlLauncherError;

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
pub(crate) fn validate(url: &str) -> Result<(), UrlLauncherError> {
    if url.is_empty() || url.len() > MAX_LEN {
        return Err(UrlLauncherError::InvalidUrl);
    }

    let bytes = url.as_bytes();
    if !bytes.is_ascii() {
        return Err(UrlLauncherError::InvalidUrl);
    }
    if bytes
        .iter()
        .any(|&byte| byte < 0x20 || byte == 0x7F || byte == b' ')
    {
        return Err(UrlLauncherError::InvalidUrl);
    }

    let Some(colon) = bytes.iter().position(|&byte| byte == b':') else {
        return Err(UrlLauncherError::InvalidUrl);
    };
    let scheme = &url[..colon];
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(UrlLauncherError::InvalidUrl);
    }
    if bytes.get(colon + 1) != Some(&b'/') || bytes.get(colon + 2) != Some(&b'/') {
        return Err(UrlLauncherError::InvalidUrl);
    }

    let rest = &bytes[colon + 3..];
    let authority_end = rest
        .iter()
        .position(|&byte| byte == b'/' || byte == b'?' || byte == b'#')
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains(&b'@') {
        return Err(UrlLauncherError::InvalidUrl);
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
                _ => return Err(UrlLauncherError::InvalidUrl),
            }
        }
        if !is_unreserved(byte) && !is_reserved(byte) {
            return Err(UrlLauncherError::InvalidUrl);
        }
        index += 1;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A URL-shaped string embedded in error text would leak the target
    /// origin into logs; this asserts none of [`UrlLauncherError`]'s fixed
    /// `Display` messages contain this stand-in, guarding against a future
    /// edit that starts interpolating a URL into one of the non-`Platform`
    /// variants.
    const MARKER: &str = "https://marker.invalid/should-not-leak";

    #[test]
    fn error_display_never_contains_a_marker_url() {
        let variants: [UrlLauncherError; 4] = [
            UrlLauncherError::InvalidUrl,
            UrlLauncherError::PlatformNotInitialized,
            UrlLauncherError::NoHandler,
            UrlLauncherError::Platform("generic backend failure".to_string()),
        ];
        for variant in variants {
            let rendered = variant.to_string();
            assert!(
                !rendered.contains(MARKER),
                "{variant:?} rendered {rendered:?}, which leaks the marker URL"
            );
        }
    }

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
