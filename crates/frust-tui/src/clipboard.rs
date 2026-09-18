//! Clipboard backend selection and writing.
//!
//! `crate::runner` used to emit a bare OSC 52 escape sequence for every copy,
//! with no environment detection and no OS-clipboard route — a terminal that
//! ignores OSC 52 silently dropped the copy. This module ports fdemon's
//! clipboard service (`fdemon-app::services::clipboard`, F0X IT LLC's own
//! product — see `docs/TUI_CODE_STANDARDS.md`) to frust-tui's shape:
//!
//! - [`detect`] is the pure decision function: given the environment and the
//!   configured [`ClipboardMode`], it picks a [`Backend`]. It never touches
//!   `std::env` or a display server itself, so it is fully unit-tested over a
//!   fake environment.
//! - [`write`] enacts one [`Backend`] against real text: the OS clipboard via
//!   `arboard` (on a detached thread, so a wedged clipboard mechanism can
//!   never hang the caller forever) or an OSC 52 escape sequence to stdout.
//!
//! `crate::runner` calls [`detect`] once at startup (stdout's TTY-ness is
//! fixed for the process's life) and keeps the returned [`Backend`] for the
//! whole run, calling [`write`] in place of the old `copy_to_clipboard`.
//!
//! The TUI runs in raw mode for its entire life, so nothing here ever prints
//! to stdout/stderr outside of the OSC 52 escape sequence itself — a failure
//! is reported only through the returned [`Result`], which the runner turns
//! into a toast (see [`super::engine::Message::Notify`]). A success still
//! carries a [`CopyOutcome`], so the runner can toast when an oversized OSC 52
//! payload was silently shortened instead of failing outright.
//!
//! Two hardening details, both answering deferred review minors:
//!
//! - [`write_system`]'s detached `arboard` thread can outlive the
//!   [`SYSTEM_CLIPBOARD_TIMEOUT`] wait (a wedged X11 selection owner can take
//!   far longer than 2s to unwedge). Without a cancellation signal, that
//!   stale thread can call `set_text` long after the caller gave up and
//!   already fell back to OSC 52 — silently clobbering whatever the user
//!   copied next with the abandoned, stale payload. An `Arc<AtomicBool>`
//!   "still wanted" flag closes that window: the caller clears it the moment
//!   it times out, and the thread checks it immediately before `set_text`,
//!   dropping the payload instead of applying it once abandoned.
//! - [`write_osc52`] caps the raw payload at [`OSC52_MAX_BYTES`] so a huge
//!   paste cannot blow past what real terminals accept for one control
//!   sequence; an oversized payload is truncated at a char boundary and
//!   reported back as [`CopyOutcome::Truncated`] rather than silently losing
//!   data or failing outright.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

/// How the runner should pick a clipboard backend, from the `FRUST_TUI_CLIPBOARD`
/// environment variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMode {
    /// Detect the environment: OS clipboard on a desktop session, OSC 52 over
    /// SSH or on a headless terminal (the default).
    Auto,
    /// Always use the OS clipboard (`arboard`), never falling back.
    System,
    /// Always emit an OSC 52 escape sequence, never touching the OS clipboard.
    Osc52,
    /// Copy is disabled outright.
    Off,
}

impl ClipboardMode {
    /// Parse `FRUST_TUI_CLIPBOARD`'s value: `"system"`, `"osc52"`, or `"off"`;
    /// anything else — including unset (`None`) — is [`ClipboardMode::Auto`].
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("system") => Self::System,
            Some("osc52") => Self::Osc52,
            Some("off") => Self::Off,
            _ => Self::Auto,
        }
    }
}

/// The concrete clipboard backend [`detect`] chose, ready for [`write`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The OS clipboard via `arboard`. [`write`] falls back to a plain
    /// (non-screen-wrapped) OSC 52 write on the same call if construction or
    /// the write itself fails or times out.
    System,
    /// An OSC 52 escape sequence to stdout. `screen` selects GNU-screen's
    /// chunked DCS wrapping over the plain sequence tmux and ordinary
    /// terminals take raw.
    Osc52 {
        /// Whether the terminal stack needs GNU-screen's DCS chunking.
        screen: bool,
    },
    /// Copy cannot work in this environment (or was explicitly turned off);
    /// every [`write`] call fails with `reason`.
    Disabled {
        /// Why copy is unavailable — surfaced verbatim in the startup toast.
        reason: &'static str,
    },
}

/// Map the configured [`ClipboardMode`] and the detected environment to a
/// concrete [`Backend`].
///
/// `env` is an injectable lookup (tests use a fake map instead of mutating
/// process env, which races with parallel tests) and `stdout_is_tty` is
/// `crate::runner`'s own `io::stdout().is_terminal()` reading, taken once at
/// startup — both are pure inputs, so this function touches nothing itself
/// and is fully unit-testable.
///
/// Decision table (mirrors fdemon's `detect::choose_backend`):
///
/// - `Off` → always [`Backend::Disabled`].
/// - `System` → always [`Backend::System`] (an explicit choice never falls
///   back to OSC 52 on construction failure — that failure is instead
///   reported by [`write`]).
/// - `Osc52` → always [`Backend::Osc52`], forced even without a TTY.
/// - `Auto`:
///   - an SSH session (`SSH_TTY`/`SSH_CONNECTION`/`SSH_CLIENT` set) with
///     stdout a TTY → [`Backend::Osc52`] — the user's clipboard lives on
///     their local machine, which only OSC 52 (relayed by the terminal) can
///     reach, even when `DISPLAY` is set via X forwarding;
///   - a display server is reachable (`DISPLAY`/`WAYLAND_DISPLAY` on Linux;
///     always true on macOS/Windows) → [`Backend::System`];
///   - no display server but stdout is a TTY (headless box, local console)
///     → [`Backend::Osc52`];
///   - neither → [`Backend::Disabled`].
pub fn detect(
    env: impl Fn(&str) -> Option<String>,
    stdout_is_tty: bool,
    mode: ClipboardMode,
) -> Backend {
    let set = |name: &str| env(name).map(|v| !v.is_empty()).unwrap_or(false);
    let ssh = set("SSH_TTY") || set("SSH_CONNECTION") || set("SSH_CLIENT");
    // tmux sets TERM=screen-256color by default but forwards raw OSC 52
    // itself; only treat this as GNU screen when not inside tmux.
    let term_is_screen = env("TERM")
        .map(|t| t.starts_with("screen"))
        .unwrap_or(false);
    let screen = (set("STY") || term_is_screen) && !set("TMUX");
    let display = cfg!(any(target_os = "macos", target_os = "windows"))
        || set("DISPLAY")
        || set("WAYLAND_DISPLAY");

    match mode {
        ClipboardMode::Off => Backend::Disabled {
            reason: "copy disabled (FRUST_TUI_CLIPBOARD=off)",
        },
        ClipboardMode::System => Backend::System,
        ClipboardMode::Osc52 => Backend::Osc52 { screen },
        ClipboardMode::Auto => {
            if ssh && stdout_is_tty {
                Backend::Osc52 { screen }
            } else if display {
                Backend::System
            } else if stdout_is_tty {
                Backend::Osc52 { screen }
            } else {
                Backend::Disabled {
                    reason: "no display server and stdout is not a terminal",
                }
            }
        }
    }
}

/// How long [`write`] waits for the OS clipboard to respond before giving up
/// and falling back to OSC 52. The `arboard::Clipboard::new()`/`set_text`
/// call runs on a detached thread that is never joined — abandoning the wait
/// here does not kill it, it simply stops blocking the caller on it.
const SYSTEM_CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(2);

/// What [`write`] actually did with the requested text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOutcome {
    /// The whole text was copied unchanged.
    Complete,
    /// The text exceeded [`OSC52_MAX_BYTES`] and was truncated to `kept_bytes`
    /// before copying — OSC 52 only, since the OS clipboard has no such
    /// limit. The caller should toast this rather than treat it as silent
    /// success.
    Truncated {
        /// How many bytes of the original (UTF-8) text were actually copied.
        kept_bytes: usize,
    },
}

/// Write `text` to the clipboard [`detect`] chose.
///
/// Never blocks the caller longer than [`SYSTEM_CLIPBOARD_TIMEOUT`] even when
/// the OS clipboard mechanism itself hangs (observed on X11 against an
/// unresponsive selection owner): [`Backend::System`] constructs and writes
/// `arboard::Clipboard` on a detached `std::thread`, and this call abandons
/// the wait — falling back to a plain OSC 52 write on the same (calling)
/// thread — the moment either the timeout elapses or the thread itself
/// reports failure. The abandoned thread is signalled to drop its payload
/// rather than apply it late (see [`write_system`]'s doc), so it can never
/// clobber a clipboard write that happened after the fallback.
///
/// The TUI runs in raw mode for its whole life: this function and everything
/// it calls must never `println!`/`eprintln!`. A failure is reported only
/// through the returned `Err`; a success carries a [`CopyOutcome`] so the
/// caller can toast a truncation. Both are turned into a toast by the caller.
pub fn write(backend: Backend, text: &str) -> Result<CopyOutcome, String> {
    match backend {
        Backend::Disabled { reason } => Err(reason.to_string()),
        Backend::Osc52 { screen } => write_osc52(text, screen),
        Backend::System => write_system(text)
            .map(|()| CopyOutcome::Complete)
            .or_else(|_| write_osc52(text, false)),
    }
}

/// [`Backend::System`]'s half of [`write`]: construct `arboard::Clipboard`
/// and set `text` on a detached thread, waiting at most
/// [`SYSTEM_CLIPBOARD_TIMEOUT`].
///
/// The thread and the caller share a "still wanted" flag: `write_system`
/// clears it the instant the wait times out (before falling back to OSC 52),
/// and the thread checks it immediately before calling `set_text`, dropping
/// the payload rather than applying it if the flag is already clear. Without
/// this, a `Clipboard::new()`/`set_text` pair that is still wedged past the
/// timeout can complete arbitrarily late and silently overwrite whatever a
/// later, successful copy already placed on the clipboard.
fn write_system(text: &str) -> Result<(), String> {
    write_system_with(text, SYSTEM_CLIPBOARD_TIMEOUT, |owned, still_wanted| {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| {
                if still_wanted.load(Ordering::Acquire) {
                    clipboard.set_text(owned)
                } else {
                    // Abandoned: the caller already timed out and fell back
                    // to OSC 52. Applying this payload now would clobber
                    // whatever the clipboard holds since then.
                    Ok(())
                }
            })
            .map_err(|e| e.to_string())
    })
}

/// The cancellable-thread protocol behind [`write_system`], generic over the
/// actual write so tests can inject a fake slow writer instead of touching
/// the real OS clipboard.
///
/// Spawns a detached thread running `write_fn(text, still_wanted)`, waits at
/// most `timeout` for it to report a result, and on timeout clears
/// `still_wanted` before returning the timeout error — `write_fn` is
/// responsible for checking the flag immediately before performing its
/// side effect and skipping it once cleared.
fn write_system_with(
    text: &str,
    timeout: Duration,
    write_fn: impl FnOnce(String, &AtomicBool) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let (done_tx, done_rx) = mpsc::channel();
    let owned = text.to_string();
    let still_wanted = Arc::new(AtomicBool::new(true));
    let thread_wanted = Arc::clone(&still_wanted);
    // Deliberately detached (not joined): a wedged clipboard mechanism must
    // never hang the caller, and joining would do exactly that. The thread
    // sending into a channel nobody is listening to any more once this
    // function has already timed out and returned is harmless — `send`
    // simply reports the (ignored) disconnect.
    std::thread::spawn(move || {
        let result = write_fn(owned, &thread_wanted);
        let _ = done_tx.send(result);
    });
    match done_rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            still_wanted.store(false, Ordering::Release);
            Err("system clipboard timed out".to_string())
        }
    }
}

/// Maximum raw (pre-base64) text bytes an OSC 52 write accepts before
/// truncating.
///
/// fdemon (`fdemon-app::services::clipboard::osc52::MAX_TEXT_BYTES`) caps
/// this at 74_994 bytes, derived so the whole *encoded* control sequence
/// (base64 expands ~4/3, plus the 7-byte header and 1-byte BEL) stays under
/// xterm's 100_000-byte limit on one OSC 52 sequence. This cap is applied to
/// the raw payload directly instead, at a round 100_000 bytes — a looser
/// bound chosen for simplicity, since the cap here exists to bound an
/// egregious paste rather than to hit xterm's figure exactly; terminals
/// tolerate the resulting encoded sequence running somewhat past 100_000
/// bytes for payloads near the cap.
pub const OSC52_MAX_BYTES: usize = 100_000;

/// Bound a single `write_all` call to this many bytes at a time when writing
/// the OSC 52 sequence to stdout — see [`write_osc52`]'s doc for why.
const OSC52_WRITE_CHUNK_BYTES: usize = 4096;

/// [`Backend::Osc52`]'s half of [`write`]: emit `ESC ] 52 ; c ; <base64> BEL`
/// to stdout — the same fd the TUI renders to — DCS-chunked in 76-byte pieces
/// when `screen` (GNU screen does not understand OSC 52 but passes DCS
/// contents through unchanged). `text` is truncated to [`OSC52_MAX_BYTES`]
/// first when oversized; the result reports that as [`CopyOutcome::Truncated`]
/// rather than failing.
///
/// This write stays synchronous on the calling thread rather than moving to
/// a detached thread the way [`write_system`] does. `crate::runner` applies
/// effects and calls `terminal.draw()` from the same event-loop iteration
/// with nothing else concurrently touching stdout, so a synchronous write
/// here is already ordered against every frame — a detached thread would
/// give up that guarantee: the next `terminal.draw()` could run on another
/// OS thread microseconds later, and a payload near the cap needs more than
/// one `write()` syscall to land (typical pipe/tty buffers are far smaller
/// than 100_000 bytes), so its later syscalls could interleave with a
/// concurrently-running frame flush on the same fd with no lock between
/// them. `write_system`'s OS-clipboard call carries no such risk (`arboard`
/// never touches stdout) and can genuinely hang for seconds, which is why
/// only that half moved off-thread. Writing in bounded
/// [`OSC52_WRITE_CHUNK_BYTES`]-sized pieces here (rather than one call for
/// the whole buffer) keeps each individual `write_all` short without
/// weakening that ordering guarantee, since every chunk still lands before
/// this function returns.
fn write_osc52(text: &str, screen: bool) -> Result<CopyOutcome, String> {
    let (payload, outcome) = truncate_to_osc52_cap(text);
    let seq = osc52_sequence(payload, screen);
    let mut out = std::io::stdout();
    for chunk in seq.chunks(OSC52_WRITE_CHUNK_BYTES) {
        out.write_all(chunk)
            .map_err(|e| format!("OSC 52 clipboard write failed: {e}"))?;
    }
    out.flush()
        .map_err(|e| format!("OSC 52 clipboard write failed: {e}"))?;
    Ok(outcome)
}

/// Truncate `text` to at most [`OSC52_MAX_BYTES`], backing up to a char
/// boundary, and report whether truncation happened.
fn truncate_to_osc52_cap(text: &str) -> (&str, CopyOutcome) {
    if text.len() <= OSC52_MAX_BYTES {
        return (text, CopyOutcome::Complete);
    }
    // Cannot underflow: byte 0 is always a char boundary (and a UTF-8 char
    // spans at most 4 bytes, so this backs up at most 3).
    let mut end = OSC52_MAX_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], CopyOutcome::Truncated { kept_bytes: end })
}

/// DCS chunk payload size used for GNU screen wrapping (matches go-osc52 and
/// hterm; safe across screen versions).
const SCREEN_CHUNK_BYTES: usize = 76;

/// Build the complete OSC 52 byte sequence that sets the terminal clipboard
/// to `text`, DCS-wrapped in [`SCREEN_CHUNK_BYTES`]-byte chunks when `screen`.
fn osc52_sequence(text: &str, screen: bool) -> Vec<u8> {
    let osc = format!("\u{1b}]52;c;{}\u{7}", base64_encode(text.as_bytes()));
    if !screen {
        return osc.into_bytes();
    }
    // Screen relays DCS contents verbatim to the outer terminal, but caps
    // DCS length — chunk the whole OSC sequence at 76 bytes, each chunk
    // framed as ESC P <chunk> ESC \.
    let mut out = Vec::with_capacity(osc.len() + (osc.len() / SCREEN_CHUNK_BYTES + 1) * 4);
    for chunk in osc.as_bytes().chunks(SCREEN_CHUNK_BYTES) {
        out.extend_from_slice(b"\x1bP");
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// Minimal standard base64 (no dependency), moved from `crate::runner`'s
/// former `copy_to_clipboard`/`base64_encode` pair unchanged — the plain
/// (non-screen) OSC 52 sequence for identical input is byte-for-byte what it
/// always was.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(b2 & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a [`Backend`]-detecting environment lookup from a synthetic
    /// `(name, value)` list, mirroring fdemon's `detect::tests::env_from`.
    fn env_from(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect();
        move |name: &str| vars.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }

    // ─── explicit modes ──────────────────────────────────────────────────

    #[test]
    fn mode_off_is_disabled_regardless_of_environment() {
        let backend = detect(env_from(&[("DISPLAY", ":0")]), true, ClipboardMode::Off);
        assert_eq!(
            backend,
            Backend::Disabled {
                reason: "copy disabled (FRUST_TUI_CLIPBOARD=off)",
            }
        );
    }

    #[test]
    fn mode_system_never_falls_back_at_detect_time() {
        let backend = detect(env_from(&[]), false, ClipboardMode::System);
        assert_eq!(backend, Backend::System);
    }

    #[test]
    fn mode_osc52_forced_even_without_tty_or_display() {
        let backend = detect(env_from(&[]), false, ClipboardMode::Osc52);
        assert_eq!(backend, Backend::Osc52 { screen: false });
    }

    #[test]
    fn mode_osc52_uses_screen_wrapping_inside_gnu_screen() {
        let backend = detect(
            env_from(&[("STY", "1234.pts-0")]),
            true,
            ClipboardMode::Osc52,
        );
        assert_eq!(backend, Backend::Osc52 { screen: true });
    }

    // ─── auto mode ───────────────────────────────────────────────────────

    #[test]
    fn auto_ssh_with_tty_prefers_osc52() {
        let backend = detect(
            env_from(&[("SSH_TTY", "/dev/pts/3")]),
            true,
            ClipboardMode::Auto,
        );
        assert_eq!(backend, Backend::Osc52 { screen: false });
    }

    #[test]
    fn auto_ssh_with_x11_forwarding_still_prefers_osc52() {
        // DISPLAY is set via X forwarding, but the user's clipboard is on
        // their local machine — only OSC 52 reaches it.
        let backend = detect(
            env_from(&[
                ("SSH_CONNECTION", "1.2.3.4 22 5.6.7.8 22"),
                ("DISPLAY", ":10"),
            ]),
            true,
            ClipboardMode::Auto,
        );
        assert_eq!(backend, Backend::Osc52 { screen: false });
    }

    #[test]
    fn auto_ssh_inside_gnu_screen_uses_screen_wrapping() {
        let backend = detect(
            env_from(&[("SSH_CLIENT", "1.2.3.4 1 22"), ("STY", "1.pts-1")]),
            true,
            ClipboardMode::Auto,
        );
        assert_eq!(backend, Backend::Osc52 { screen: true });
    }

    #[test]
    fn auto_ssh_without_tty_falls_through_to_the_display_check() {
        // stdout redirected: OSC 52 cannot reach the terminal.
        let backend = detect(
            env_from(&[("SSH_TTY", "/dev/pts/3"), ("DISPLAY", ":0")]),
            false,
            ClipboardMode::Auto,
        );
        assert_eq!(backend, Backend::System);
    }

    #[test]
    fn auto_desktop_session_uses_the_system_clipboard() {
        let backend = detect(env_from(&[("DISPLAY", ":0")]), true, ClipboardMode::Auto);
        assert_eq!(backend, Backend::System);
    }

    #[test]
    fn auto_wayland_session_uses_the_system_clipboard() {
        let backend = detect(
            env_from(&[("WAYLAND_DISPLAY", "wayland-0")]),
            true,
            ClipboardMode::Auto,
        );
        assert_eq!(backend, Backend::System);
    }

    #[test]
    fn auto_headless_tty_uses_osc52() {
        // Local console / headless box: no display server, stdout is a tty.
        let backend = detect(env_from(&[]), true, ClipboardMode::Auto);
        assert_eq!(backend, Backend::Osc52 { screen: false });
    }

    #[test]
    fn auto_no_display_no_tty_is_disabled() {
        let backend = detect(env_from(&[]), false, ClipboardMode::Auto);
        assert_eq!(
            backend,
            Backend::Disabled {
                reason: "no display server and stdout is not a terminal",
            }
        );
    }

    #[test]
    fn auto_tmux_with_screen_term_is_not_gnu_screen() {
        // tmux sets TERM=screen-256color by default; it must get raw OSC 52
        // (which tmux forwards), never screen's DCS wrapping (which tmux
        // would silently drop).
        let backend = detect(
            env_from(&[
                ("SSH_TTY", "/dev/pts/3"),
                ("TERM", "screen-256color"),
                ("TMUX", "/tmp/tmux-1000/default,42,0"),
            ]),
            true,
            ClipboardMode::Auto,
        );
        assert_eq!(backend, Backend::Osc52 { screen: false });
    }

    // ─── FRUST_TUI_CLIPBOARD parsing ─────────────────────────────────────

    #[test]
    fn parse_recognises_each_explicit_value() {
        assert_eq!(ClipboardMode::parse(Some("system")), ClipboardMode::System);
        assert_eq!(ClipboardMode::parse(Some("osc52")), ClipboardMode::Osc52);
        assert_eq!(ClipboardMode::parse(Some("off")), ClipboardMode::Off);
    }

    #[test]
    fn parse_defaults_to_auto_on_unset_or_unrecognised() {
        assert_eq!(ClipboardMode::parse(None), ClipboardMode::Auto);
        assert_eq!(ClipboardMode::parse(Some("")), ClipboardMode::Auto);
        assert_eq!(ClipboardMode::parse(Some("bogus")), ClipboardMode::Auto);
    }

    // ─── OSC 52 sequence bytes ───────────────────────────────────────────

    #[test]
    fn plain_sequence_exact_bytes() {
        // base64("hello") == "aGVsbG8="
        assert_eq!(osc52_sequence("hello", false), b"\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn plain_sequence_empty_text_clears_the_clipboard() {
        assert_eq!(osc52_sequence("", false), b"\x1b]52;c;\x07");
    }

    #[test]
    fn screen_short_sequence_is_one_dcs_chunk() {
        assert_eq!(osc52_sequence("hi", true), b"\x1bP\x1b]52;c;aGk=\x07\x1b\\");
    }

    #[test]
    fn screen_long_sequence_chunked_at_76_bytes() {
        let text = "x".repeat(300);
        let out = osc52_sequence(&text, true);

        let osc = format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()));
        let expected_chunks = osc.as_bytes().chunks(76).count();

        let mut inner = Vec::new();
        let mut rest: &[u8] = &out;
        let mut chunks = 0;
        while !rest.is_empty() {
            assert_eq!(&rest[..2], b"\x1bP", "each chunk must open with DCS");
            let end = rest
                .windows(2)
                .position(|w| w == b"\x1b\\")
                .expect("each chunk must close with ST");
            inner.extend_from_slice(&rest[2..end]);
            rest = &rest[end + 2..];
            chunks += 1;
        }
        assert_eq!(chunks, expected_chunks);
        assert_eq!(inner, osc.as_bytes());
    }

    // ─── base64 ────────────────────────────────────────────────────────────

    #[test]
    fn base64_encodes_with_standard_alphabet_and_padding() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }

    // ─── OSC 52 payload cap ──────────────────────────────────────────────

    #[test]
    fn cap_boundary_below_limit_is_unchanged() {
        let text = "a".repeat(OSC52_MAX_BYTES - 1);
        let (payload, outcome) = truncate_to_osc52_cap(&text);
        assert_eq!(payload.len(), OSC52_MAX_BYTES - 1);
        assert_eq!(outcome, CopyOutcome::Complete);
    }

    #[test]
    fn cap_boundary_at_limit_is_unchanged() {
        let text = "a".repeat(OSC52_MAX_BYTES);
        let (payload, outcome) = truncate_to_osc52_cap(&text);
        assert_eq!(payload.len(), OSC52_MAX_BYTES);
        assert_eq!(outcome, CopyOutcome::Complete);
    }

    #[test]
    fn cap_boundary_above_limit_truncates() {
        let text = "a".repeat(OSC52_MAX_BYTES + 1000);
        let (payload, outcome) = truncate_to_osc52_cap(&text);
        assert_eq!(payload.len(), OSC52_MAX_BYTES);
        assert_eq!(
            outcome,
            CopyOutcome::Truncated {
                kept_bytes: OSC52_MAX_BYTES
            }
        );
    }

    #[test]
    fn cap_truncation_backs_up_to_a_char_boundary() {
        // Fill so a 4-byte emoji straddles the cap.
        let mut text = "a".repeat(OSC52_MAX_BYTES - 2);
        text.push('🦀'); // 4 bytes, crosses OSC52_MAX_BYTES
        text.push_str(&"b".repeat(100));
        let (payload, outcome) = truncate_to_osc52_cap(&text);
        assert!(payload.len() <= OSC52_MAX_BYTES);
        assert_eq!(payload.len(), OSC52_MAX_BYTES - 2);
        assert!(payload.is_char_boundary(payload.len()));
        assert_eq!(
            outcome,
            CopyOutcome::Truncated {
                kept_bytes: OSC52_MAX_BYTES - 2
            }
        );
    }

    #[test]
    fn write_osc52_reports_truncated_outcome_for_oversized_text() {
        let text = "a".repeat(OSC52_MAX_BYTES + 1000);
        let outcome = write_osc52(&text, false).unwrap();
        assert_eq!(
            outcome,
            CopyOutcome::Truncated {
                kept_bytes: OSC52_MAX_BYTES
            }
        );
    }

    #[test]
    fn write_osc52_reports_complete_for_text_within_the_cap() {
        let outcome = write_osc52("hello", false).unwrap();
        assert_eq!(outcome, CopyOutcome::Complete);
    }

    // ─── write_system cancellation protocol ─────────────────────────────

    #[test]
    fn abandoned_write_is_not_applied_after_timeout() {
        let applied = Arc::new(AtomicBool::new(false));
        let inner_applied = Arc::clone(&applied);
        let done = Arc::new(AtomicBool::new(false));
        let inner_done = Arc::clone(&done);
        let result = write_system_with(
            "text",
            Duration::from_millis(20),
            move |_owned, still_wanted| {
                // Simulate a wedged clipboard mechanism: sleep well past the
                // caller's timeout before checking whether it's still wanted.
                std::thread::sleep(Duration::from_millis(150));
                if still_wanted.load(Ordering::Acquire) {
                    inner_applied.store(true, Ordering::Release);
                }
                inner_done.store(true, Ordering::Release);
                Ok(())
            },
        );
        assert_eq!(result, Err("system clipboard timed out".to_string()));

        // Wait for the detached thread to actually run its check.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !done.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(done.load(Ordering::Acquire), "writer thread never ran");
        assert!(
            !applied.load(Ordering::Acquire),
            "abandoned write must not be applied once the caller gave up"
        );
    }

    #[test]
    fn write_within_timeout_is_applied_and_succeeds() {
        let applied = Arc::new(AtomicBool::new(false));
        let inner_applied = Arc::clone(&applied);
        let result = write_system_with(
            "text",
            Duration::from_secs(1),
            move |_owned, still_wanted| {
                if still_wanted.load(Ordering::Acquire) {
                    inner_applied.store(true, Ordering::Release);
                }
                Ok(())
            },
        );
        assert_eq!(result, Ok(()));
        assert!(applied.load(Ordering::Acquire));
    }

    #[test]
    fn write_system_with_propagates_the_writer_error() {
        let result = write_system_with("text", Duration::from_secs(1), |_owned, _still_wanted| {
            Err("boom".to_string())
        });
        assert_eq!(result, Err("boom".to_string()));
    }

    // ─── write() ─────────────────────────────────────────────────────────

    #[test]
    fn write_disabled_backend_fails_with_its_reason() {
        let err = write(
            Backend::Disabled {
                reason: "copy disabled (FRUST_TUI_CLIPBOARD=off)",
            },
            "text",
        )
        .unwrap_err();
        assert_eq!(err, "copy disabled (FRUST_TUI_CLIPBOARD=off)");
    }

    /// Exercises the real OS clipboard via `arboard` — not run in CI, which
    /// has no display server (the `System` backend would either hang the
    /// full `SYSTEM_CLIPBOARD_TIMEOUT` or silently fall back to OSC 52,
    /// neither of which asserts anything meaningful headless). Run manually
    /// on a machine with a reachable clipboard mechanism.
    #[test]
    #[ignore = "touches the real OS clipboard; no display server in CI"]
    fn write_system_backend_reaches_the_real_clipboard() {
        write(Backend::System, "frust-tui clipboard test").unwrap();
    }
}
