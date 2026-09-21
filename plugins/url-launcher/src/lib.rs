//! `frust-url-launcher`: a minimal platform plugin that opens an absolute
//! `http`/`https` URL in the device's default external handler — Android
//! `Intent(ACTION_VIEW)`/`startActivity` via plain JNI, iOS
//! `UIApplication.openURL(_:options:completionHandler:)` via `objc2-ui-kit`,
//! desktop (macOS/Linux/Windows) the platform opener (`open`/`xdg-open`/
//! `ShellExecuteW`).
//!
//! # Charter: a platform plugin
//!
//! Like [`frust-haptics`](../frust_haptics/index.html) and
//! `frust-clipboard`/`frust-secure-storage`/`frust-shared-preferences`, this
//! is a **platform plugin** (see `docs/ARCHITECTURE.md`'s Module Structure):
//! it depends on `frust-plugin` plus FFI crates only, and carries **no other
//! `frust-*` framework dependency**. An app adds this crate to its own
//! `Cargo.toml` alongside `frust`, the Flutter-pubspec model; the `frust`
//! facade does not depend on or re-export it.
//!
//! # Fire-and-forget, best-effort
//!
//! [`UrlLauncher::open_external`] is a one-shot imperative action, not a
//! state query — there is nothing to read back. A caller with no interest in
//! *why* a launch failed is expected to ignore the `Result` in the common
//! case; the `Result` exists for the caller that does want to distinguish an
//! invalid URL from a not-yet-initialized platform from "no app can handle
//! this" from a genuine backend failure (see [`UrlLauncherError`]). Every
//! backend degrades gracefully rather than panicking.
//!
//! On Android, a pre-`nativeInitPlatform` scaffold (an old app shell that
//! predates the plugin platform-handle install) reports
//! [`UrlLauncherError::PlatformNotInitialized`] rather than panicking — the
//! same contract [`frust_plugin::android::with_jni_env`] gives every other
//! platform plugin. On iOS, [`UrlLauncher::open_external`] always returns
//! `Ok(())` once validation passes: the actual `openURL:` call is dispatched
//! asynchronously to the main queue and runs *after* this function has
//! already returned, so a failure there (no handler, a suspended app, …) is
//! unobservable to the caller — there is nothing left to report back on that
//! path (see the `apple` module's own doc — Android-/iOS-only modules are
//! not linked here since they compile out of a non-mobile `cargo doc`).
//!
//! # One validator, every backend
//!
//! [`UrlLauncher::open_external`] runs the `url` module's `validate` before
//! routing to any backend — the single [`UrlLauncherError::InvalidUrl`] gate
//! every platform shares, closing the well-known
//! `javascript:`/`intent:`/`tel:`/`file:`-scheme and userinfo-phishing URL
//! classes before a single platform API is touched (see the `url` module's
//! doc for the full rule table).
//!
//! # Backends
//!
//! [`UrlLauncher::open_external`] routes by `#[cfg(target_os = ...)]` to one
//! of three real backends: `android` (`Intent(ACTION_VIEW)`, plain JNI — no
//! Kotlin/Gradle module, see that module's doc for why one isn't needed),
//! `apple` (iOS only — `UIApplication.openURL(_:options:completionHandler:)`,
//! dispatched onto the main queue), and `desktop` (macOS, Linux, Windows —
//! the platform's own URL opener). A build target with none of the above
//! (tvOS, wasm, …) has no backend module compiled in at all and reports
//! [`UrlLauncherError::Platform`] rather than failing to compile — unlike
//! `frust-haptics`'s [`Unavailability`](../frust_haptics/enum.Unavailability.html)
//! enum, this crate's error enum has no dedicated "unsupported platform"
//! variant (the task's fixed four-variant API), so that fallback reuses the
//! catch-all `Platform` variant with a fixed message instead of inventing a
//! fifth.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod apple;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
mod desktop;
mod url;

/// Errors from a [`UrlLauncher::open_external`] call.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant rather than only displaying it. None of these `Display` messages
/// ever echo the URL back (see each backend module's own doc for why —
/// notably `android`'s JNI-exception handling and `desktop`'s spawn-error
/// handling, both of which report only a class name / `io::ErrorKind`).
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum UrlLauncherError {
    /// `url` failed the `url` module's `validate` rule table — not an absolute
    /// `http`/`https` URL, or an otherwise malformed/unsafe one (userinfo,
    /// stray control bytes, a bad percent-escape, …).
    #[error("url launcher: not an absolute http/https URL")]
    InvalidUrl,

    /// The Android host shell never installed the `(JavaVM, Context)`
    /// platform handles this crate's Android backend needs
    /// (`frust-plugin`'s pre-init state) — an old scaffold predating
    /// `nativeInitPlatform`. Never a panic; the caller degrades gracefully.
    #[error("url launcher platform not initialized")]
    PlatformNotInitialized,

    /// No installed app can handle an `http`/`https` `ACTION_VIEW` intent
    /// (Android `ActivityNotFoundException`), or, on Windows,
    /// `ShellExecuteW` reports no association for the URL
    /// (`SE_ERR_NOASSOC`/`SE_ERR_ASSOCINCOMPLETE`). On macOS/Linux this
    /// variant means only that the opener binary itself (`open`/`xdg-open`)
    /// is missing from `PATH` (a spawn-time `NotFound`) — a spawn that
    /// succeeds but whose opener then runs and reports no association of
    /// its own (e.g. `xdg-open`'s nonzero exit code when nothing is
    /// registered) is unobservable to the caller on those two platforms:
    /// `open_external` has already returned `Ok(())` by the time that exit
    /// status is available, on a detached thread with no channel back into
    /// the finished call (see the `desktop` module's own doc). This mirrors
    /// this crate doc's already-documented iOS post-dispatch
    /// unobservability.
    #[error("url launcher: no handler for http/https URLs on this device")]
    NoHandler,

    /// A backend-specific failure that isn't a not-yet-initialized/
    /// no-handler condition — a platform JNI/process-spawn/`ShellExecuteW`
    /// error this crate doesn't otherwise classify.
    #[error("url launcher error: {0}")]
    Platform(String),
}

/// The platform URL-launcher entry point.
///
/// Carries no state — [`Self::open_external`] is a plain associated
/// function, matching `frust-haptics`'s `Haptics`.
pub struct UrlLauncher;

impl UrlLauncher {
    /// Open `url` in the device's default external handler, fire-and-forget
    /// (the crate doc's *Fire-and-forget, best-effort* section) — a caller
    /// with no interest in *why* nothing opened is expected to ignore the
    /// `Result`.
    ///
    /// # Errors
    /// [`UrlLauncherError::InvalidUrl`] if `url` is not an absolute
    /// `http`/`https` URL (see the `url` module's `validate` rule table) —
    /// checked before any platform API is touched. Otherwise, per platform: Android
    /// [`UrlLauncherError::PlatformNotInitialized`] for an old scaffold
    /// predating `nativeInitPlatform`, or [`UrlLauncherError::NoHandler`]/
    /// [`UrlLauncherError::Platform`] for a genuine JNI failure; iOS always
    /// `Ok(())` (see the crate doc's *Fire-and-forget* section for why a
    /// post-dispatch failure is unobservable); desktop
    /// [`UrlLauncherError::NoHandler`] when no opener is installed, else
    /// [`UrlLauncherError::Platform`] for a genuine spawn/`ShellExecuteW`
    /// failure.
    pub fn open_external(url: &str) -> Result<(), UrlLauncherError> {
        url::validate(url)?;

        #[cfg(target_os = "android")]
        {
            android::open_external(url)
        }
        #[cfg(target_os = "ios")]
        {
            apple::open_external(url)
        }
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            desktop::open_external(url)
        }
        // Total-cover fallback: any target not one of the four arms above
        // (tvOS, wasm, …) has no backend module compiled in at all — report
        // it as a typed error rather than failing to compile with a
        // confusing "no arm produced a value" error (matches
        // `frust-haptics`/`frust-clipboard`'s own dispatch functions, modulo
        // this crate's narrower error enum — see the crate doc's *Backends*
        // section).
        #[cfg(not(any(
            target_os = "android",
            target_os = "ios",
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        )))]
        {
            Err(UrlLauncherError::Platform(
                "url launcher not available on this platform".to_string(),
            ))
        }
    }
}
