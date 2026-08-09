//! The devtools discovery-line contract: a server announces its listening
//! port with one printed line built from [`DISCOVERY_PREFIX`], and
//! [`parse_discovery_line`] is the single parser tooling reuses to find it —
//! the same shared-marker-string pattern as `frust-drive`'s
//! `FRUST-SIGNING-FALLBACK` token (`crates/frust-drive/src/android_build/
//! signing.rs`): one constant, not two independently-typed literals that
//! could drift.

/// The exact substring a devtools server's discovery line carries, followed
/// immediately by a bare decimal port number.
pub const DISCOVERY_PREFIX: &str = "frust-devtools listening on ";

/// Finds [`DISCOVERY_PREFIX`] anywhere in `line` — a **substring** search,
/// not a line-start anchor — and parses the run of ASCII digits
/// immediately following it as a port number.
///
/// Substring (not anchored) search is deliberate: a host logger commonly
/// prepends its own prefix before app output reaches it (Android `logcat`'s
/// `MM-DD HH:MM:SS.mmm PID TID L Tag: `, a desktop process supervisor's
/// timestamp, …), so tooling must find the marker wherever it lands in the
/// line, not only at column 0.
///
/// Returns `None` if the prefix is absent, or is not immediately followed
/// by at least one digit.
pub fn parse_discovery_line(line: &str) -> Option<u16> {
    let idx = line.find(DISCOVERY_PREFIX)?;
    let rest = &line[idx + DISCOVERY_PREFIX.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_line() {
        assert_eq!(
            parse_discovery_line("frust-devtools listening on 54321"),
            Some(54321)
        );
    }

    #[test]
    fn parses_with_trailing_text() {
        assert_eq!(
            parse_discovery_line("frust-devtools listening on 54321\n"),
            Some(54321)
        );
        assert_eq!(
            parse_discovery_line("frust-devtools listening on 54321 (waiting for client)"),
            Some(54321)
        );
    }

    #[test]
    fn parses_with_logcat_style_prefix() {
        let line = "08-10 12:00:00.123  1234  5678 I Frust   : frust-devtools listening on 8123";
        assert_eq!(parse_discovery_line(line), Some(8123));
    }

    #[test]
    fn parses_with_generic_timestamp_and_tag_prefix() {
        let line = "[2026-08-10T12:00:00Z] app: frust-devtools listening on 65000";
        assert_eq!(parse_discovery_line(line), Some(65000));
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
}
