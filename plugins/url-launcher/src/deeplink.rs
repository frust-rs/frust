//! Validator for native map-app URIs (`geo:`, `maps:`, `comgooglemaps:`,
//! `google.navigation:`), kept separate from the http/https guard in `url`
//! so that guard is never relaxed.
//!
//! Rules: total length at most [`MAX_LEN`]; scheme (case-insensitive) on the
//! allowlist; the remainder holds only ASCII alphanumerics and
//! `? & = , . + - %`, with every `%` followed by two hex digits. There is no
//! authority: `@`, `:`, `;`, `\`, whitespace and control bytes are rejected.
//! The one exception is a leading `//` for `maps` and `comgooglemaps`
//! (`maps://?daddr=...`), which must be followed by `?` or the end.

use crate::UrlLauncherError;

/// Maximum accepted URI length in bytes.
const MAX_LEN: usize = 2048;

/// Schemes whose remainder may begin with `//`.
const SLASH_SCHEMES: [&str; 2] = ["maps", "comgooglemaps"];

/// Every accepted scheme.
const SCHEMES: [&str; 4] = ["geo", "maps", "comgooglemaps", "google.navigation"];

/// Validate a native map URI. See the module doc for the rule set.
pub(crate) fn validate_maps_native(uri: &str) -> Result<(), UrlLauncherError> {
    let bad = || UrlLauncherError::InvalidUrl;
    if uri.is_empty() || uri.len() > MAX_LEN || !uri.is_ascii() {
        return Err(bad());
    }
    let (scheme, rest) = uri.split_once(':').ok_or_else(bad)?;
    let scheme = scheme.to_ascii_lowercase();
    if !SCHEMES.contains(&scheme.as_str()) {
        return Err(bad());
    }
    let rest = match rest.strip_prefix("//") {
        Some(after) if SLASH_SCHEMES.contains(&scheme.as_str()) => {
            if !after.is_empty() && !after.starts_with('?') {
                return Err(bad());
            }
            after
        }
        _ => rest,
    };
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'%' => {
                let ok = bytes
                    .get(i + 1..i + 3)
                    .is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit));
                if !ok {
                    return Err(bad());
                }
                i += 2;
            }
            b'?' | b'&' | b'=' | b',' | b'.' | b'+' | b'-' => {}
            _ if b.is_ascii_alphanumeric() => {}
            _ => return Err(bad()),
        }
        i += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_native_map_uris() {
        for u in [
            "geo:0,0?q=Pool",
            "geo:37.78,-122.41",
            "GEO:1,2?q=a%20b",
            "maps://?daddr=1.5,2.5",
            "maps:?daddr=1,2",
            "comgooglemaps://?daddr=1,2&directionsmode=driving",
            "google.navigation:q=1.5,-2.5",
        ] {
            assert!(validate_maps_native(u).is_ok(), "{u}");
        }
    }

    #[test]
    fn rejects_bad_uris() {
        let long = format!("geo:0,0?q={}", "a".repeat(MAX_LEN));
        for u in [
            "",
            "geo",
            "geo://evil@host",
            "geo://host",
            "maps://host/path",
            "javascript:alert(1)",
            "https://maps.apple.com/?q=1",
            "tel:123",
            "geo:0,0?q=a b",
            "geo:0,0?q=a\nb",
            "geo:0,0\0",
            "geo:0,0?q=%zz",
            "geo:0,0?q=%4",
            "geo:0,0;u=1",
            "geo:a:b",
            "geo:0,0?q=\u{e9}",
            long.as_str(),
        ] {
            assert!(
                matches!(validate_maps_native(u), Err(UrlLauncherError::InvalidUrl)),
                "{u:?}"
            );
        }
    }
}
