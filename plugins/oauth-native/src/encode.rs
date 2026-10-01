//! Percent-encoding and query decoding shared by the authorization URL
//! builder, the callback parser and the token-grant form bodies.
//!
//! Two encoders: [`encode_component`] for a URI query component (RFC 3986
//! §2.3 — everything but `ALPHA DIGIT - . _ ~` is `%XX`, so a space becomes
//! `%20`), and [`form_encode`] for an `application/x-www-form-urlencoded`
//! body (the same set, except a space becomes `+`). One decoder,
//! [`parse_query`], which treats `+` as a space and refuses a malformed
//! `%` escape or a value that does not decode to UTF-8.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// Every byte except RFC 3986 `unreserved` (`ALPHA DIGIT - . _ ~`).
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encodes `value` for use as a URI query name or value: every byte
/// outside the RFC 3986 unreserved set becomes an uppercase `%XX` (UTF-8
/// bytes individually), so a space is `%20` and a `+` is `%2B`.
pub(crate) fn encode_component(value: &str) -> String {
    utf8_percent_encode(value, COMPONENT).to_string()
}

/// Encodes one value for an `application/x-www-form-urlencoded` body: the
/// unreserved set is kept, a space becomes `+`, everything else is `%XX`.
fn form_value(value: &str) -> String {
    // A literal `%` in the input is itself encoded to `%25`, so every `%20`
    // in the component encoding came from a space.
    encode_component(value).replace("%20", "+")
}

/// Joins `pairs` into an `application/x-www-form-urlencoded` body
/// (`name=value&name=value`), in the given order.
pub(crate) fn form_encode<'a, I>(pairs: I) -> String
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut out = String::new();
    for (name, value) in pairs {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&form_value(name));
        out.push('=');
        out.push_str(&form_value(value));
    }
    out
}

/// A query string could not be decoded: a `%` not followed by two hex
/// digits, an empty parameter name, or a decoded byte sequence that is not
/// UTF-8. Carries nothing about the input, so it can never leak it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecodeError;

/// Decodes one query name or value: `+` is a space, `%XX` is the byte `XX`,
/// and the result must be UTF-8.
pub(crate) fn decode_component(raw: &str) -> Result<String, DecodeError> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hi = bytes.get(i + 1).copied().and_then(hex_value);
                let lo = bytes.get(i + 2).copied().and_then(hex_value);
                match (hi, lo) {
                    (Some(hi), Some(lo)) => out.push((hi << 4) | lo),
                    _ => return Err(DecodeError),
                }
                i += 3;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| DecodeError)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decodes a query string (the part after `?`, without it) into its
/// `(name, value)` pairs, in order. Empty segments (`a=1&&b=2`, a trailing
/// `&`) are skipped; a segment without `=` is a name with an empty value
/// (the WHATWG URL rule). An empty name or a bad escape is a [`DecodeError`].
pub(crate) fn parse_query(query: &str) -> Result<Vec<(String, String)>, DecodeError> {
    let mut pairs = Vec::new();
    for segment in query.split('&') {
        if segment.is_empty() {
            continue;
        }
        let (raw_name, raw_value) = segment.split_once('=').unwrap_or((segment, ""));
        let name = decode_component(raw_name)?;
        if name.is_empty() {
            return Err(DecodeError);
        }
        pairs.push((name, decode_component(raw_value)?));
    }
    Ok(pairs)
}

/// RFC 6749 §4.1.2.1 / §5.2 `error` and `error_description` charset:
/// `%x20-21 / %x23-5B / %x5D-7E` (printable ASCII except `"` and `\`).
/// `error` additionally needs at least one character — callers check that.
pub(crate) fn is_error_text(value: &str) -> bool {
    value
        .bytes()
        .all(|b| matches!(b, 0x20..=0x21 | 0x23..=0x5B | 0x5D..=0x7E))
}

/// RFC 6749 §4.1.2.1 / §5.2 `error_uri` charset: `%x21 / %x23-5B / %x5D-7E`
/// (the error-text set without the space).
pub(crate) fn is_error_uri(value: &str) -> bool {
    value
        .bytes()
        .all(|b| matches!(b, 0x21 | 0x23..=0x5B | 0x5D..=0x7E))
}

/// Whether `value` consists only of RFC 3986 unreserved characters
/// (`[A-Za-z0-9-._~]`).
pub(crate) fn is_unreserved(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(encode_component("AZaz09-._~"), "AZaz09-._~");
        assert_eq!(encode_component("a b"), "a%20b");
        assert_eq!(encode_component("a+b"), "a%2Bb");
        assert_eq!(encode_component("a/b:c"), "a%2Fb%3Ac");
        assert_eq!(encode_component("100%"), "100%25");
        assert_eq!(encode_component("?&=#"), "%3F%26%3D%23");
    }

    #[test]
    fn component_encodes_non_ascii_as_utf8_bytes() {
        assert_eq!(encode_component("café"), "caf%C3%A9");
        assert_eq!(
            encode_component("openid 日本"),
            "openid%20%E6%97%A5%E6%9C%AC"
        );
    }

    #[test]
    fn form_encoding_uses_plus_for_space() {
        assert_eq!(
            form_encode([("scope", "openid profile"), ("k", "a+b/c:d%")]),
            "scope=openid+profile&k=a%2Bb%2Fc%3Ad%25"
        );
        assert_eq!(form_encode([("x", "é")]), "x=%C3%A9");
        assert_eq!(form_encode(std::iter::empty::<(&str, &str)>()), "");
    }

    #[test]
    fn decoding_treats_plus_as_space_and_decodes_escapes() {
        assert_eq!(decode_component("a+b%20c%2B").unwrap(), "a b c+");
        assert_eq!(decode_component("caf%c3%a9").unwrap(), "café");
    }

    #[test]
    fn decoding_rejects_malformed_escapes_and_bad_utf8() {
        assert_eq!(decode_component("%"), Err(DecodeError));
        assert_eq!(decode_component("%2"), Err(DecodeError));
        assert_eq!(decode_component("%zz"), Err(DecodeError));
        assert_eq!(decode_component("%FF"), Err(DecodeError));
    }

    #[test]
    fn query_parsing_splits_pairs_in_order() {
        assert_eq!(
            parse_query("a=1&&b=x+y&c&").unwrap(),
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "x y".to_string()),
                ("c".to_string(), String::new()),
            ]
        );
        assert_eq!(parse_query("").unwrap(), Vec::new());
        assert_eq!(parse_query("=v"), Err(DecodeError));
        assert_eq!(parse_query("a=%G0"), Err(DecodeError));
    }

    #[test]
    fn error_charsets_follow_rfc_6749() {
        assert!(is_error_text("access_denied"));
        assert!(is_error_text("The user said no!"));
        assert!(!is_error_text("quote\"d"));
        assert!(!is_error_text("back\\slash"));
        assert!(!is_error_text("line\nbreak"));
        assert!(!is_error_text("é"));
        assert!(is_error_uri("https://as.example/errors#denied"));
        assert!(!is_error_uri("has space"));
    }
}
