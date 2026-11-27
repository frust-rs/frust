//! The panic hook: a Rust panic reaches logcat with its message and location.
//!
//! The release profile builds with `panic = "abort"`, so an unhandled panic ends the process
//! with a bare SIGABRT and the default hook's stderr output goes nowhere on Android. The hook
//! installed here logs one line at error priority under the `frust` tag, then chains to the hook
//! that was in place before it (the default one, or whatever an app or test installed), so
//! the abort (or, in a debug build, the unwind that `frust_shell_common::guard` catches) follows
//! exactly as before.
//!
//! The line is `frust: panic at <file>:<line>:<col> on thread '<name>': <message>`. The message
//! is the payload when it is a `&str` or `String`, else a fixed placeholder, and it is cut at
//! [`MAX_PAYLOAD_BYTES`]. The formatting is pure and host-tested; [`install_once`] is called
//! from `JNI_OnLoad`, right after the logger comes up.

use std::any::Any;
use std::panic::{self, PanicHookInfo};
use std::sync::Once;

/// The longest payload kept; a longer one is cut on a char boundary and marked.
pub(crate) const MAX_PAYLOAD_BYTES: usize = 1024;

/// Stands in for a payload that is neither `&str` nor `String`.
pub(crate) const NON_STRING_PAYLOAD: &str = "<non-string panic payload>";

/// The thread name when the panicking thread has none.
const UNNAMED_THREAD: &str = "<unnamed>";

/// The location text when the panic carries none.
const UNKNOWN_LOCATION: &str = "<unknown location>";

/// The payload as text: `&str` and `String` verbatim, anything else the placeholder. Newlines
/// become a literal `\n` so the report stays one logcat line, and the result is truncated.
fn payload_text(payload: &(dyn Any + Send)) -> String {
    let raw = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or(NON_STRING_PAYLOAD);
    let mut out = String::with_capacity(raw.len().min(MAX_PAYLOAD_BYTES) + 16);
    for ch in raw.chars() {
        let add = if ch == '\n' { 2 } else { ch.len_utf8() };
        if out.len() + add > MAX_PAYLOAD_BYTES {
            out.push_str("...");
            break;
        }
        if ch == '\n' {
            out.push_str("\\n");
        } else {
            out.push(ch);
        }
    }
    out
}

/// The one-line report for a panic.
pub(crate) fn format_panic(
    location: Option<(&str, u32, u32)>,
    thread: Option<&str>,
    payload: &(dyn Any + Send),
) -> String {
    let thread = thread.unwrap_or(UNNAMED_THREAD);
    let message = payload_text(payload);
    match location {
        Some((file, line, col)) => {
            format!("frust: panic at {file}:{line}:{col} on thread '{thread}': {message}")
        }
        None => format!("frust: panic at {UNKNOWN_LOCATION} on thread '{thread}': {message}"),
    }
}

fn report(info: &PanicHookInfo<'_>) -> String {
    let location = info.location().map(|l| (l.file(), l.line(), l.column()));
    let current = std::thread::current();
    format_panic(location, current.name(), info.payload())
}

/// Install the logging hook, chained to the previous one, once per `guard`. A second call on
/// the same `guard` does nothing, so the previous hook is never wrapped twice.
pub(crate) fn install_into(guard: &Once, sink: fn(&str)) {
    guard.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            sink(&report(info));
            previous(info);
        }));
    });
}

fn log_error(line: &str) {
    log::error!(target: "frust", "{line}");
}

/// Install the hook for the process, logging through the `log` facade (logcat once the Android
/// logger is up). Harmless to call again.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn install_once() {
    static INSTALLED: Once = Once::new();
    install_into(&INSTALLED, log_error);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn formats_str_payload_with_location() {
        let line = format_panic(Some(("src/a.rs", 3, 9)), Some("main"), &"boom");
        assert_eq!(line, "frust: panic at src/a.rs:3:9 on thread 'main': boom");
    }

    #[test]
    fn formats_string_payload_without_location_or_thread() {
        let line = format_panic(None, None, &String::from("bad"));
        assert_eq!(
            line,
            "frust: panic at <unknown location> on thread '<unnamed>': bad"
        );
    }

    #[test]
    fn formats_non_string_payload_with_placeholder() {
        let line = format_panic(Some(("x.rs", 1, 1)), Some("t"), &42_i32);
        assert!(line.ends_with(&format!(": {NON_STRING_PAYLOAD}")), "{line}");
    }

    #[test]
    fn newlines_stay_on_one_line() {
        let line = format_panic(None, Some("t"), &"a\nb");
        assert!(!line.contains('\n'));
        assert!(line.ends_with("a\\nb"));
    }

    #[test]
    fn truncates_long_payload_on_a_char_boundary() {
        let long = "é".repeat(MAX_PAYLOAD_BYTES);
        let text = payload_text(&long);
        assert!(text.ends_with("..."));
        assert!(text.len() <= MAX_PAYLOAD_BYTES + 3);
        let ascii = "a".repeat(MAX_PAYLOAD_BYTES * 3);
        let text = payload_text(&ascii);
        assert_eq!(text.len(), MAX_PAYLOAD_BYTES + 3);
        let exact = "a".repeat(MAX_PAYLOAD_BYTES);
        assert_eq!(payload_text(&exact), exact);
    }

    static SINK_HITS: AtomicUsize = AtomicUsize::new(0);
    static PREVIOUS_HITS: AtomicUsize = AtomicUsize::new(0);
    const MARKER: &str = "panic_hook-test-marker";
    /// Serialises the one test that swaps the process-wide hook.
    static HOOK_LOCK: Mutex<()> = Mutex::new(());

    fn sink(line: &str) {
        if line.contains(MARKER) {
            SINK_HITS.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn installing_twice_chains_exactly_once() {
        let _lock = HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let original = panic::take_hook();
        panic::set_hook(Box::new(|info| {
            if info
                .payload()
                .downcast_ref::<&str>()
                .is_some_and(|m| m.contains(MARKER))
            {
                PREVIOUS_HITS.fetch_add(1, Ordering::SeqCst);
            }
        }));
        let guard = Once::new();
        install_into(&guard, sink);
        install_into(&guard, sink);
        let _ = panic::catch_unwind(|| panic!("{}", MARKER));
        let _ = panic::catch_unwind(|| std::panic::panic_any(MARKER));
        panic::set_hook(original);
        assert_eq!(SINK_HITS.load(Ordering::SeqCst), 2);
        assert_eq!(PREVIOUS_HITS.load(Ordering::SeqCst), 1);
    }
}
