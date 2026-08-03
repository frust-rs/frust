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
            Ok(text) => Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(other) => Err(map_err(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full cross-backend conformance suite ([`crate::conformance`])
    /// against the real host clipboard. A headless CI runner or container
    /// with no reachable clipboard mechanism (no X11 display, no Wayland
    /// compositor, no `NSPasteboard`/Windows equivalent) can't run this —
    /// probe availability first and skip with a clear message rather than
    /// fail (`docs/DEVELOPMENT.md`'s Test section asks for exactly this
    /// shape for a host-dependent gate).
    #[test]
    fn conformance() {
        if let Err(e) = ArboardClipboard::new() {
            eprintln!(
                "skipping frust-clipboard desktop conformance: no clipboard \
                 mechanism available on this host ({e})"
            );
            return;
        }
        crate::conformance::run_conformance_suite(&|| Box::new(DesktopClipboard));
    }
}
