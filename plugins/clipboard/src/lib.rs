//! `frust-clipboard`: a platform-independent, synchronous plain-text
//! clipboard for frust apps — Android `ClipboardManager` via plain JNI, iOS
//! `UIPasteboard` via `objc2-ui-kit`, and macOS/Linux/Windows via `arboard`.
//!
//! # Charter: a platform plugin
//!
//! Like [`frust-secure-storage`](../frust_secure_storage/index.html) and
//! [`frust-shared-preferences`](../frust_shared_preferences/index.html),
//! this is a **platform plugin** (see `docs/ARCHITECTURE.md`'s Module
//! Structure): it depends on `frust-plugin` plus FFI crates only, and
//! carries **no other `frust-*` framework dependency**. An app adds this
//! crate to its own `Cargo.toml` alongside `frust`, the Flutter-pubspec
//! model; the `frust` facade does not depend on or re-export it.
//!
//! # v1 is a single global slot, sync, plain text only
//!
//! Unlike `SecureStorage`'s named-store-handle model, there is exactly one
//! OS clipboard — so [`Clipboard`] carries no handle to open: [`Clipboard::set_text`]/
//! [`Clipboard::get_text`] are plain associated functions. None of the three
//! platforms this crate targets documents a main-thread requirement for its
//! clipboard API (Android's `ClipboardManager`, `arboard`'s desktop
//! backends, and iOS's `UIPasteboard` — see [`apple`]'s module doc for why
//! that last one is an absence-of-hazard finding, not a documented
//! any-thread guarantee), so v1 exposes a synchronous API with nothing to
//! block on. A future byte/image payload is left to a later version; v1 is
//! `String` only, matching `frust-shared-preferences`'/`frust-secure-storage`'s
//! own v1 scope.
//!
//! [`Clipboard::get_text`] returns `Ok(None)` for an empty or non-text
//! clipboard — **and also for a clipboard whose text content is the empty
//! string** — never an error — matching Android's `getPrimaryClip` returning
//! `null` and iOS/arboard's "content not available" case. This is a
//! deliberate single story every backend and the conformance suite hold to:
//! there is no way for a caller to distinguish "never set" from "set to
//! `\"\"`" through this API, and v1 does not need one.
//!
//! # Security: sensitive content
//!
//! [`Clipboard::set_text`] moves `text` into a **world-readable OS
//! channel** — any other app with clipboard access (there is no
//! permission gate on any of the three platforms) can read it back, and on
//! Android 13+ the OS may briefly *preview* the copied text on-screen in a
//! system toast. For content a user would not want another app or a
//! screen-recording to see — copied terminal output, tokens, passwords,
//! one-time codes — use [`Clipboard::set_text_sensitive`] instead, which
//! applies each platform's best-effort sensitivity marking (see that
//! method's doc for exactly what each platform does and does not protect
//! against). Neither call encrypts the clipboard or prevents a
//! determined reader; both are best-effort hints to the OS/other apps.
//! See the plugin `README.md`'s `## Security` section for the full
//! platform-by-platform writeup.
//!
//! # Backends
//!
//! [`Clipboard::set_text`]/[`Clipboard::get_text`]/[`Clipboard::set_text_sensitive`]
//! route by `#[cfg(target_os = ...)]` to one of three real backends:
//! [`android`] (`ClipboardManager`/`ClipData`, plain JNI — no Kotlin/Gradle
//! module, see that module's doc for why one isn't needed), [`apple`] (iOS
//! only — `UIPasteboard`), and [`desktop`] (macOS, Linux, Windows —
//! `arboard`), plus a total-cover fallback arm on any other target (see
//! [`Unavailability::UnsupportedPlatform`]). All three real backends are
//! exercised by the shared [`conformance::run_conformance_suite`]
//! (`#[cfg(test)]`): host-runnable on desktop (this crate's own `cargo
//! test`), compile-gated only on mobile (`cargo check --target
//! aarch64-linux-android`/`aarch64-apple-ios -p frust-clipboard` — see
//! `docs/DEVELOPMENT.md`'s Test section).
//!
//! # Platform caveats (see the plugin `README.md` for the full writeup)
//!
//! - **Android 10+ focus gate**: `getPrimaryClip` returns `null` (so
//!   [`Clipboard::get_text`] reports `Ok(None)`) when the calling app isn't
//!   in the foreground — a documented platform privacy restriction, not a
//!   bug; do not attempt to work around it.
//! - **iOS 14+ paste banner**: reading the pasteboard shows a one-time
//!   system banner ("App pasted from Notes") — functional, not an error.
//! - **X11/Wayland clipboard lifetime (Linux)**: clipboard content set from
//!   this process is served by this process; it disappears when the process
//!   exits unless a clipboard manager is running to adopt it. This is
//!   documented `arboard`/X11 behavior (see [`desktop`]'s module doc), not a
//!   Frust bug — no lifetime workaround is installed here.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod apple;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
mod desktop;

#[cfg(test)]
mod conformance;

/// Errors from a [`Clipboard`] operation.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant (e.g. distinguishing a genuinely unavailable clipboard mechanism
/// from a one-off backend failure) rather than only displaying it. Kept
/// deliberately small — clipboard access has none of secure-storage's
/// biometric-gate failure taxonomy.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum ClipboardError {
    /// The Android host shell never installed the `(JavaVM, Context)`
    /// platform handles this crate's Android backend needs
    /// (`frust-plugin`'s pre-init state) — an old scaffold predating
    /// `nativeInitPlatform`. Never a panic; the caller degrades gracefully.
    #[error("clipboard platform not initialized")]
    PlatformNotInitialized,

    /// The clipboard mechanism itself is unavailable for the given reason
    /// (see [`Unavailability`]) — distinct from a transient per-call
    /// failure ([`Self::Platform`]).
    #[error("clipboard unavailable: {0:?}")]
    NotAvailable(Unavailability),

    /// A backend-specific failure that isn't a not-yet-initialized/
    /// unavailable condition — a platform JNI/Objective-C error, or an
    /// `arboard` failure this crate doesn't otherwise classify.
    #[error("clipboard error: {0}")]
    Platform(String),
}

/// Why the clipboard mechanism is unavailable (the
/// [`ClipboardError::NotAvailable`] payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unavailability {
    /// No clipboard mechanism is reachable in the current environment —
    /// e.g. a desktop Linux session with neither X11 nor a Wayland
    /// clipboard protocol available (`arboard`'s
    /// `Error::ClipboardNotSupported`; see [`desktop`]'s module doc), or a
    /// build target this crate has no backend module for at all (not
    /// Android/iOS/macOS/Linux/Windows — e.g. tvOS, wasm), where `Clipboard`'s
    /// associated functions report this rather than failing to compile.
    UnsupportedPlatform,
}

/// One backend implementation — [`android::AndroidClipboard`],
/// [`apple::AppleClipboard`] (iOS only), or [`desktop::DesktopClipboard`]
/// (macOS/Linux/Windows).
///
/// Crate-private and deliberately minimal: [`Clipboard`]'s public API is a
/// thin wrapper cfg-dispatching to exactly one of these per target. The
/// conformance suite ([`conformance::run_conformance_suite`],
/// `#[cfg(test)]`) exercises any backend factory uniformly, so the same
/// assertions validate every platform backend.
pub(crate) trait Backend: Send + Sync {
    /// Store `text` as the clipboard's plain-text content, overwriting
    /// whatever it held before (of any type/format).
    fn set_text(&self, text: &str) -> Result<(), ClipboardError>;
    /// The clipboard's current plain-text content, or `None` if it is empty
    /// (including a clipboard explicitly set to `""`) or holds a non-text
    /// payload. Never an error for "empty" — see the crate doc's *v1 is a
    /// single global slot* section.
    fn get_text(&self) -> Result<Option<String>, ClipboardError>;
    /// As [`Self::set_text`], plus a best-effort platform sensitivity
    /// marking — see [`Clipboard::set_text_sensitive`]'s doc for what each
    /// backend does. Default: identical to [`Self::set_text`] (the desktop
    /// backend has no platform sensitivity primitive to apply, so it never
    /// overrides this).
    fn set_text_sensitive(&self, text: &str) -> Result<(), ClipboardError> {
        self.set_text(text)
    }
}

/// The platform clipboard.
///
/// Carries no state (there is exactly one OS clipboard, unlike
/// `SecureStorage`'s named stores) — [`Self::set_text`]/[`Self::get_text`]
/// are plain associated functions.
pub struct Clipboard;

impl Clipboard {
    /// Store `text` as the clipboard's plain-text content, overwriting
    /// whatever it held before (of any type/format).
    ///
    /// # Errors
    /// On Android, [`ClipboardError::PlatformNotInitialized`] for an old
    /// scaffold predating `nativeInitPlatform`. On desktop,
    /// [`ClipboardError::NotAvailable`] if no clipboard mechanism is
    /// reachable (see [`Unavailability::UnsupportedPlatform`]), or
    /// [`ClipboardError::Platform`] for any other backend failure.
    pub fn set_text(text: &str) -> Result<(), ClipboardError> {
        #[cfg(target_os = "android")]
        {
            android::AndroidClipboard.set_text(text)
        }
        #[cfg(target_os = "ios")]
        {
            apple::AppleClipboard.set_text(text)
        }
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            desktop::DesktopClipboard.set_text(text)
        }
        // Total-cover fallback: any target not one of the four arms above
        // (tvOS, wasm, …) has no backend module compiled in at all — report
        // it as a typed unavailability rather than failing to compile with a
        // confusing "no arm produced a value" error.
        #[cfg(not(any(
            target_os = "android",
            target_os = "ios",
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        )))]
        {
            let _ = text;
            Err(ClipboardError::NotAvailable(
                Unavailability::UnsupportedPlatform,
            ))
        }
    }

    /// The clipboard's current plain-text content, or `None` if it is empty
    /// (including a clipboard explicitly set to `""`) or holds a non-text
    /// payload.
    ///
    /// # Errors
    /// As [`Self::set_text`]. On Android 10+, an unfocused caller sees
    /// `Ok(None)` rather than an error — see the crate doc's platform
    /// caveats.
    pub fn get_text() -> Result<Option<String>, ClipboardError> {
        #[cfg(target_os = "android")]
        {
            android::AndroidClipboard.get_text()
        }
        #[cfg(target_os = "ios")]
        {
            apple::AppleClipboard.get_text()
        }
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            desktop::DesktopClipboard.get_text()
        }
        // Total-cover fallback — see `set_text`'s.
        #[cfg(not(any(
            target_os = "android",
            target_os = "ios",
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        )))]
        {
            Err(ClipboardError::NotAvailable(
                Unavailability::UnsupportedPlatform,
            ))
        }
    }

    /// As [`Self::set_text`], plus a best-effort platform sensitivity
    /// marking for content the user would not want visible to another app,
    /// a clipboard-history UI, or another of the user's devices (copied
    /// terminal output, tokens, passwords, one-time codes) — see the crate
    /// doc's *Security: sensitive content* section.
    ///
    /// This is a **hint, not a guarantee**: it does not encrypt the
    /// clipboard, and any app the OS grants clipboard access to can still
    /// read the text back with [`Self::get_text`]/its own platform API.
    ///
    /// - **Android (API 33+):** sets `ClipDescription.EXTRA_IS_SENSITIVE` on
    ///   the clip description's extras, which suppresses the Android 13+
    ///   copied-text preview toast and flags clipboard-history UIs. Ignored
    ///   (a plain [`Self::set_text`]) below API 33.
    /// - **iOS:** writes via `UIPasteboard.setItems(_:options:)` with
    ///   `.localOnly: true`, which blocks Universal Clipboard/Handoff
    ///   replication to the user's other signed-in devices. No expiration
    ///   date is set (a possible future opt-in — see [`apple`]'s module
    ///   doc).
    /// - **Desktop (arboard):** behaves exactly like [`Self::set_text`] on
    ///   all three targets — but only X11/Wayland genuinely has no
    ///   sensitivity mechanism. Windows and macOS each document/convention
    ///   one this backend does not yet set (see [`desktop`]'s module doc's
    ///   *No sensitivity marking*): `arboard` exposes no custom-format
    ///   write surface, so reaching either would need `clipboard-win`'s raw
    ///   format API directly — deliberately deferred
    ///   (`docs/LIMITATIONS.md`'s `clip-desktop-sensitivity-noop`).
    ///
    /// # Errors
    /// As [`Self::set_text`].
    pub fn set_text_sensitive(text: &str) -> Result<(), ClipboardError> {
        #[cfg(target_os = "android")]
        {
            android::AndroidClipboard.set_text_sensitive(text)
        }
        #[cfg(target_os = "ios")]
        {
            apple::AppleClipboard.set_text_sensitive(text)
        }
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            desktop::DesktopClipboard.set_text_sensitive(text)
        }
        // Total-cover fallback — see `set_text`'s.
        #[cfg(not(any(
            target_os = "android",
            target_os = "ios",
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        )))]
        {
            let _ = text;
            Err(ClipboardError::NotAvailable(
                Unavailability::UnsupportedPlatform,
            ))
        }
    }
}
