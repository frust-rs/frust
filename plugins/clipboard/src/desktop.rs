//! The desktop (macOS/Linux/Windows) [`Backend`] — `arboard`.
//!
//! `#[cfg(any(target_os = "macos", target_os = "linux", target_os =
//! "windows"))]`: macOS routes here rather than sharing [`crate::apple`] —
//! `UIPasteboard` (UIKit) doesn't exist on macOS, and `arboard`'s own macOS
//! backend already drives `NSPasteboard` via `objc2-app-kit`, so there is
//! nothing macOS-specific left for this crate to write (see [`crate::apple`]'s
//! module doc for the split rationale).
//!
//! # Construct-per-call
//!
//! `arboard::Clipboard::new()` is documented cheap and side-effect-free to
//! call repeatedly ("Any number of `Clipboard` instances are allowed to
//! exist at a single point in time"), so this backend opens one per
//! operation rather than caching a long-lived instance — simplest, and
//! avoids holding an OS clipboard handle open for the app's whole lifetime
//! on platforms (Windows) where the clipboard is a single global object only
//! one thread may hold at a time.
//!
//! # X11/Wayland clipboard lifetime (Linux)
//!
//! Per `arboard`'s own documentation (`SetExtLinux::wait`'s doc, not enabled
//! by this crate — see below): on X11 and Wayland the clipboard's content is
//! served live by whichever process last claimed ownership of the
//! selection; when *that* process exits, the content disappears unless a
//! clipboard manager (`klipper`, `xfce4-clipman`, `CopyQ`, …) is running to
//! adopt it first. This is documented `arboard`/X11 platform behavior, not a
//! Frust bug — this backend does **not** install a lifetime workaround (a
//! background thread holding a `Clipboard` open forever, or calling
//! `SetExtLinux::wait`/`wait_until` after every `set_text` to block until
//! overwritten): a short-lived CLI/desktop-preview process that sets the
//! clipboard and exits may lose that content on a system with no clipboard
//! manager running, exactly as any other `arboard` consumer would.
//!
//! This crate also does not enable `arboard`'s `wayland-data-control`
//! feature (native Wayland selection support via `wl-clipboard-rs`); a
//! Wayland session without XWayland compatibility may therefore be unable to
//! reach the clipboard at all — a documented, deliberately-deferred gap, not
//! enabled here to keep the dependency graph minimal (see this crate's
//! `Cargo.toml`).
//!
//! # No sensitivity marking
//!
//! This backend does not override [`Backend::set_text_sensitive`]'s
//! default — it behaves exactly like [`Backend::set_text`] on all three
//! desktop targets, but for different reasons per platform. X11/Wayland
//! genuinely has no sensitivity mechanism to hook. Windows and macOS do:
//! Windows documents the `CanIncludeInClipboardHistory`,
//! `CanUploadToCloudClipboard`, and
//! `ExcludeClipboardContentFromMonitorProcessing` clipboard formats
//! precisely to keep a clip out of Win+V clipboard history and Cloud
//! Clipboard sync (the same cross-device replication channel `.localOnly`
//! closes on iOS — see [`crate::apple`]'s module doc); macOS has the
//! community convention `org.nspasteboard.ConcealedType` (not
//! OS-documented, unlike Windows' formats). Neither is set here — `arboard`
//! exposes no custom-format write surface, so reaching either would
//! require `clipboard-win`'s raw format API directly. Deliberately
//! deferred, not implemented (`docs/LIMITATIONS.md`'s
//! `clip-desktop-sensitivity-noop`).

use arboard::Clipboard as ArboardClipboard;

use crate::{Backend, ClipboardError, Unavailability};

/// The desktop backend. Stateless — every operation opens a fresh
/// `arboard::Clipboard` (see the module doc's *Construct-per-call*).
pub(crate) struct DesktopClipboard;

/// Map an `arboard::Error` to this crate's error taxonomy.
/// `ClipboardNotSupported` (no reachable clipboard mechanism — e.g. a
/// headless Linux session with neither X11 nor Wayland) is the one variant
/// with a matching [`Unavailability`] reason; every other failure (
/// `ClipboardOccupied`, `ConversionFailure`, `Unknown`) folds into
/// [`ClipboardError::Platform`]. `ContentNotAvailable` is handled by each
/// call site directly (it means "empty", not an error — see
/// [`Backend::get_text`]'s contract), so it never reaches this function from
/// [`DesktopClipboard::get_text`].
fn map_err(err: arboard::Error) -> ClipboardError {
    match err {
        arboard::Error::ClipboardNotSupported => {
            ClipboardError::NotAvailable(Unavailability::UnsupportedPlatform)
        }
        other => ClipboardError::Platform(other.to_string()),
    }
}

impl Backend for DesktopClipboard {
    fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let mut clipboard = ArboardClipboard::new().map_err(map_err)?;
        clipboard.set_text(text).map_err(map_err)
    }

    fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        let mut clipboard = ArboardClipboard::new().map_err(map_err)?;
        match clipboard.get_text() {
            // Crate-wide contract (see `crate::Clipboard::get_text`'s doc):
            // an empty string reads back as `Ok(None)`, the same as
            // `ContentNotAvailable` below — there is no way for a caller to
            // distinguish "never set" from "set to `\"\"`" through this API.
            Ok(text) if text.is_empty() => Ok(None),
            Ok(text) => Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(other) => Err(map_err(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opt-in env var gating the test below: it exercises the **real host
    /// clipboard** and overwrites whatever the developer had copied,
    /// leaving a test string behind when it finishes. A plain `cargo test
    /// --workspace`/`-p frust-clipboard` must never clobber a developer's
    /// clipboard as a side effect, so the test no-ops (printing this
    /// variable's name) unless it is explicitly set. Tripwire command (also
    /// in this crate's `README.md` and `Cargo.toml`): `FRUST_CLIPBOARD_TESTS=1
    /// cargo test -p frust-clipboard`.
    const CLIPBOARD_TESTS_ENV: &str = "FRUST_CLIPBOARD_TESTS";

    fn clipboard_tests_opted_in() -> bool {
        std::env::var(CLIPBOARD_TESTS_ENV).as_deref() == Ok("1")
    }

    /// The full cross-backend conformance suite ([`crate::conformance`]),
    /// plus a `set_text_sensitive`-aliases-`set_text` check (desktop has no
    /// sensitivity primitive to apply — see the module doc's *No
    /// sensitivity marking* — so it should read back identically), against
    /// the real host clipboard. Both live in **one** test function rather
    /// than two: two tests each opening/writing the real OS clipboard would
    /// run concurrently under `cargo test`'s default parallelism, and doing
    /// so was observed to crash (`arboard`'s macOS `NSPasteboard` backend is
    /// not documented safe for concurrent access from independent
    /// `Clipboard` instances) — one test, one thread, no race.
    ///
    /// **Destructive** (see this module's opt-in gate above) — skipped by
    /// default. When opted in, a headless CI runner or container with no
    /// reachable clipboard mechanism (no X11 display, no Wayland
    /// compositor, no `NSPasteboard`/Windows equivalent) still can't run
    /// this — probe availability next and skip with a clear message rather
    /// than fail (`docs/DEVELOPMENT.md`'s Test section asks for exactly
    /// this shape for a host-dependent gate).
    #[test]
    fn conformance() {
        if !clipboard_tests_opted_in() {
            eprintln!(
                "skipping frust-clipboard desktop conformance: destructive \
                 (overwrites the real host clipboard) — opt in with \
                 `{CLIPBOARD_TESTS_ENV}=1 cargo test -p frust-clipboard`"
            );
            return;
        }
        if let Err(e) = ArboardClipboard::new() {
            eprintln!(
                "skipping frust-clipboard desktop conformance: no clipboard \
                 mechanism available on this host ({e})"
            );
            return;
        }
        crate::conformance::run_conformance_suite(&|| Box::new(DesktopClipboard));

        let backend = DesktopClipboard;
        backend
            .set_text_sensitive("frust-clipboard sensitive alias check")
            .unwrap();
        assert_eq!(
            backend.get_text().unwrap(),
            Some("frust-clipboard sensitive alias check".to_string())
        );
    }
}
