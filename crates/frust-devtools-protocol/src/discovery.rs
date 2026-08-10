//! The devtools discovery-line contract: a server announces its listening
//! port — and, since auth landed, the per-process token a client must present
//! at `handshake` — with one printed line built from [`DISCOVERY_PREFIX`].
//! [`format_discovery_line`] writes it and [`parse_discovery_line`] reads it
//! back, so the two sides share one definition rather than two independently
//! typed literals that could drift (the same shared-marker-string pattern as
//! `frust-drive`'s `FRUST-SIGNING-FALLBACK` token,
//! `crates/frust-drive/src/android_build/signing.rs`).
//!
//! # Shape
//!
//! ```text
//! frust-devtools listening on 54321 token 6f1c…c0de
//! frust-devtools listening on 54321          (a server running with auth off)
//! ```
//!
//! The token is **whitespace-delimited** and always last, so a host logger that
//! appends its own trailing text cannot swallow it, and a line from a server
//! that requires no token still parses — [`Discovery::token`] is simply `None`.

/// The exact substring a devtools server's discovery line carries, followed
/// immediately by a bare decimal port number.
pub const DISCOVERY_PREFIX: &str = "frust-devtools listening on ";

/// The exact substring a shell logs (via [`format_failure_line`]) when the
/// in-app devtools service is compiled in but could not start — a bind refused
/// by the OS, an ephemeral-port exhaustion, a runtime that would not build. The
/// human reason follows immediately.
///
/// Tooling greps for this the same way it greps for [`DISCOVERY_PREFIX`], so a
/// session that will never announce a port turns "waiting for a discovery
/// line…" into the concrete reason instead of an eternal silent wait. Shared
/// here so the logging side and the parsing side cannot drift.
pub const FAILURE_PREFIX: &str = "frust-devtools: service did not start: ";

/// What separates the port from the auth token on a discovery line. Private:
/// both sides go through [`format_discovery_line`]/[`parse_discovery_line`]
/// rather than splicing the marker themselves.
const TOKEN_MARKER: &str = " token ";

/// A parsed discovery line: everything a client needs to open an authenticated
/// connection.
///
/// `token` is `None` for a line written by a server running with auth disabled
/// (`ServiceConfig`'s switch) or by a build predating the token — a client with
/// no token simply sends none and lets the server decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub port: u16,
    pub token: Option<String>,
}

/// Builds the one discovery line a server logs on start. The server side's
/// only formatter — see the module doc for the shape.
pub fn format_discovery_line(port: u16, token: Option<&str>) -> String {
    match token {
        Some(token) => format!("{DISCOVERY_PREFIX}{port}{TOKEN_MARKER}{token}"),
        None => format!("{DISCOVERY_PREFIX}{port}"),
    }
}

/// Finds [`DISCOVERY_PREFIX`] anywhere in `line` — a **substring** search,
/// not a line-start anchor — and parses the port (and optional token) that
/// follow it.
///
/// Substring (not anchored) search is deliberate: a host logger commonly
/// prepends its own prefix before app output reaches it (Android `logcat`'s
/// `MM-DD HH:MM:SS.mmm PID TID L Tag: `, a desktop process supervisor's
/// timestamp, …), so tooling must find the marker wherever it lands in the
/// line, not only at column 0.
///
/// Returns `None` if the prefix is absent, or is not immediately followed
/// by at least one digit. A line with no token — or with the marker but
/// nothing after it — still yields its port, with `token: None`.
pub fn parse_discovery_line(line: &str) -> Option<Discovery> {
    let idx = line.find(DISCOVERY_PREFIX)?;
    let rest = &line[idx + DISCOVERY_PREFIX.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let port: u16 = digits.parse().ok()?;
    let token = rest[digits.len()..]
        .strip_prefix(TOKEN_MARKER)
        .map(|after| {
            after
                .chars()
                .take_while(|c| !c.is_whitespace())
                .collect::<String>()
        })
        .filter(|token| !token.is_empty());
    Some(Discovery { port, token })
}

/// Builds the one line a shell logs when the devtools service fails to start.
/// The shell side's only formatter — pair of [`parse_failure_line`].
pub fn format_failure_line(reason: &str) -> String {
    format!("{FAILURE_PREFIX}{reason}")
}

/// Finds [`FAILURE_PREFIX`] anywhere in `line` (a substring search, for the
/// same host-logger-prefix reason as [`parse_discovery_line`]) and returns the
/// trimmed human reason after it, or `None` if the marker is absent or nothing
/// non-empty follows it.
pub fn parse_failure_line(line: &str) -> Option<&str> {
    let idx = line.find(FAILURE_PREFIX)?;
    let reason = line[idx + FAILURE_PREFIX.len()..].trim_end();
    (!reason.is_empty()).then_some(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port_of(line: &str) -> Option<u16> {
        parse_discovery_line(line).map(|d| d.port)
    }

    #[test]
    fn failure_line_round_trips() {
        let line = format_failure_line("Connection refused (os error 111)");
        assert_eq!(
            parse_failure_line(&line),
            Some("Connection refused (os error 111)")
        );
    }

    #[test]
    fn parses_failure_line_with_logcat_prefix() {
        let line = "08-10 10:22:16.394 27868 27868 W frust   : frust_shell_common::devtools: \
                    frust-devtools: service did not start: Connection refused (os error 111)";
        assert_eq!(
            parse_failure_line(line),
            Some("Connection refused (os error 111)")
        );
    }

    #[test]
    fn failure_line_absent_or_empty_is_none() {
        assert_eq!(parse_failure_line("some unrelated log line"), None);
        assert_eq!(parse_failure_line(FAILURE_PREFIX), None);
    }

    #[test]
    fn parses_bare_line() {
        assert_eq!(
            parse_discovery_line("frust-devtools listening on 54321"),
            Some(Discovery {
                port: 54321,
                token: None
            })
        );
    }

    #[test]
    fn parses_with_trailing_text() {
        assert_eq!(port_of("frust-devtools listening on 54321\n"), Some(54321));
        assert_eq!(
            port_of("frust-devtools listening on 54321 (waiting for client)"),
            Some(54321)
        );
    }

    #[test]
    fn parses_with_logcat_style_prefix() {
        let line = "08-10 12:00:00.123  1234  5678 I Frust   : frust-devtools listening on 8123";
        assert_eq!(port_of(line), Some(8123));
    }

    #[test]
    fn parses_with_generic_timestamp_and_tag_prefix() {
        let line = "[2026-08-10T12:00:00Z] app: frust-devtools listening on 65000";
        assert_eq!(port_of(line), Some(65000));
    }

    #[test]
    fn returns_none_when_prefix_absent() {
        assert_eq!(parse_discovery_line("some unrelated log line"), None);
    }

    #[test]
    fn returns_none_when_no_digits_follow_prefix() {
        assert_eq!(
            parse_discovery_line("frust-devtools listening on not-a-port"),
            None
        );
    }

    #[test]
    fn returns_none_on_empty_line() {
        assert_eq!(parse_discovery_line(""), None);
    }

    #[test]
    fn format_and_parse_round_trip_with_a_token() {
        let line = format_discovery_line(54321, Some("0123456789abcdef0123456789abcdef"));
        assert_eq!(
            parse_discovery_line(&line),
            Some(Discovery {
                port: 54321,
                token: Some("0123456789abcdef0123456789abcdef".to_string()),
            })
        );
    }

    #[test]
    fn format_and_parse_round_trip_without_a_token() {
        let line = format_discovery_line(1234, None);
        assert_eq!(
            parse_discovery_line(&line),
            Some(Discovery {
                port: 1234,
                token: None
            })
        );
    }

    #[test]
    fn a_tokened_line_survives_a_logger_prefix_and_trailing_text() {
        let line = format!(
            "08-10 12:00:00.123  1234  5678 I Frust   : {} (waiting)",
            format_discovery_line(8123, Some("deadbeef"))
        );
        assert_eq!(
            parse_discovery_line(&line),
            Some(Discovery {
                port: 8123,
                token: Some("deadbeef".to_string()),
            })
        );
    }

    #[test]
    fn a_truncated_token_marker_still_yields_the_port() {
        // Tolerance, not strictness: the port is recoverable, and a client
        // that presents no token simply gets rejected at handshake.
        assert_eq!(
            parse_discovery_line("frust-devtools listening on 9000 token "),
            Some(Discovery {
                port: 9000,
                token: None
            })
        );
    }

    #[test]
    fn trailing_text_that_is_not_the_token_marker_is_not_a_token() {
        assert_eq!(
            parse_discovery_line("frust-devtools listening on 9000 tokenish stuff"),
            Some(Discovery {
                port: 9000,
                token: None
            })
        );
    }
}
