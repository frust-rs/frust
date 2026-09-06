//! Making client-supplied text safe to echo.
//!
//! Two places quote a string the *client* chose back at a human: the launch
//! note naming an ignored `projectRoot`, and the connected-client registry the
//! host renders in its UI. Both land somewhere a terminal escape sequence
//! would be interpreted rather than shown — a DAP Debug Console, and (since
//! this server is embedded) the host workbench's own raw-mode terminal. A
//! client that sends `\x1b[2J` in a path must not get to repaint someone's
//! screen with it.

/// The stand-in every control character is echoed as.
///
/// A visible placeholder rather than a silent removal: "the client sent
/// something unprintable here" is itself information, and a path that quietly
/// loses characters reads like the server mangled it.
const REPLACEMENT: char = '\u{FFFD}';

/// `text` with every control character replaced by [`REPLACEMENT`].
///
/// Covers ESC (the ANSI-injection vector), CR/LF (which would forge extra
/// console lines), and the C1 range — `char::is_control` is exactly the
/// Unicode `Cc` category, so nothing in that class survives.
pub(crate) fn console_safe(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { REPLACEMENT } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(console_safe("/home/me/app"), "/home/me/app");
        // Non-ASCII is not control text: a path with accents stays readable.
        assert_eq!(console_safe("/home/mé/äpp"), "/home/mé/äpp");
    }

    #[test]
    fn an_escape_sequence_cannot_reach_the_terminal() {
        let sanitized = console_safe("/tmp/\u{1b}[2Jgone");
        assert!(!sanitized.contains('\u{1b}'), "{sanitized}");
        assert_eq!(sanitized, "/tmp/\u{FFFD}[2Jgone");
    }

    #[test]
    fn newlines_and_carriage_returns_cannot_forge_a_console_line() {
        let sanitized = console_safe("a\r\nLaunching desktop\u{7f}");
        assert_eq!(sanitized, "a\u{FFFD}\u{FFFD}Launching desktop\u{FFFD}");
    }
}
