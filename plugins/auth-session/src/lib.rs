//! `frust-auth-session`: an RFC 8252-style "OAuth for native apps" in-app
//! browser session — Android Custom Tabs / macOS+iOS
//! `ASWebAuthenticationSession` presented over the request's `https` URL,
//! resolving to the redirect the identity provider sends back to
//! `callback_scheme://…` (or the user cancelling).
//!
//! # Charter: a platform plugin
//!
//! Like [`frust-haptics`](../frust_haptics/index.html) and
//! [`frust-url-launcher`](../frust_url_launcher/index.html), this is a
//! **platform plugin** (see `docs/ARCHITECTURE.md`'s Module Structure): it
//! depends on `frust-plugin` plus FFI crates only, and carries **no other
//! `frust-*` framework dependency**. An app adds this crate to its own
//! `Cargo.toml` alongside `frust`, the Flutter-pubspec model; the `frust`
//! facade does not depend on or re-export it.
//!
//! # RFC 8252 context
//!
//! [RFC 8252](https://www.rfc-editor.org/rfc/rfc8252) ("OAuth 2.0 for Native
//! Apps") requires a native app to run its authorization-code flow through
//! the platform's own in-app browser tab — never an embedded webview — so
//! the identity provider's session cookies, autofill, and password-manager
//! integration are shared with the user's regular browser. [`AuthSession`]
//! is that browser tab: it presents `req.url` (an identity provider's
//! `/authorize` endpoint), waits for the provider to redirect back to
//! `req.callback_scheme://…`, and resolves with that redirect URL — or
//! [`AuthSessionOutcome::Cancelled`] if the user dismisses the tab. Exchanging
//! the resulting authorization code for tokens is the caller's own HTTP
//! call; this crate's job ends at handing back the raw callback URL.
//!
//! # The awaitable-future contract
//!
//! [`AuthSession::start`] returns a plain [`core::future::Future`] — no
//! `futures`/`tokio` dependency (the platform-plugin charter above). Poll it
//! from the UI thread's local executor (`frust::spawn_local`, per
//! `crates/frust-reactive/src/executor.rs`'s `ForgeExecutor::spawn_local`)
//! or from any other executor a caller prefers: resolution is driven by the
//! platform's own main thread (Android's main looper delivering the Custom
//! Tabs redirect intent, Apple's completion block dispatched onto the main
//! queue), not by whichever thread happens to be polling, so nothing about
//! this future requires the UI thread specifically — it only *has* to reach
//! the UI thread if the caller's own continuation touches UI state.
//!
//! # Exactly one live session
//!
//! At most one [`AuthSession`] can be in flight at a time, process-wide: a
//! second [`AuthSession::start`] call while one is still live resolves
//! immediately to [`AuthSessionError::Busy`] rather than queuing or
//! replacing the first (an in-app browser tab is a modal, single-instance
//! UI surface on every backend platform). The slot releases the moment the
//! live session resolves — by outcome, by error, or by the backend module
//! dropping its resolution handle without ever calling back (this crate's
//! `oneshot` module's own drop-without-send fallback) — so a stuck session
//! can never wedge every future one behind it forever.
//!
//! # Platform notes
//!
//! Android and Apple (iOS + macOS) both ship a real backend module
//! (`android`, `apple`) — as of this crate's originating task, both are
//! stubs that resolve every session to
//! [`AuthSessionError::Platform`]`("backend not implemented")` immediately,
//! so the crate compiles and this host-testable core is usable on every
//! target from day one; the platform backend cards replace them with a real
//! `CustomTabsIntent`/`ASWebAuthenticationSession` implementation without
//! touching this file's public API. Every other target (desktop Linux,
//! Windows, macOS-without-`ASWebAuthenticationSession`… — see `unsupported`)
//! has no platform authentication user agent at all and always resolves
//! [`AuthSessionError::NoHandler`]; see that module's own doc for the
//! recommended fallback.
//!
//! # The Android double-delivery note
//!
//! On Android, the callback URL Custom Tabs redirects back into the app
//! reaches the platform **twice**: once as the redirect this crate's Android
//! backend resolves [`AuthSession::start`]'s future with, and once more as an
//! ordinary deep link the app's own `frust::deep_links()` stream also
//! observes (the OS has no way to know only this crate wants that intent).
//! Treat the future as the single authoritative source for an auth-session
//! callback and ignore `callback_scheme` in the app's own deep-link
//! handling — acting on both delivers the same authorization code twice.
//!
//! # The URL never appears in a `Display`/`Debug` string
//!
//! Neither [`AuthSessionRequest`]'s hand-written [`core::fmt::Debug`] impl
//! nor any [`AuthSessionError`] variant's `Display` message ever echoes
//! `req.url` or the resolved callback URL back into a log line — an
//! authorization code or an identity-provider session detail can ride along
//! in either one, so this crate never interpolates a URL into anything that
//! might get logged (see [`AuthSessionRequest`]'s own doc and this crate's
//! `url` module's copied-from-`frust-url-launcher` test of the same rule).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, MutexGuard};
use std::task::{Context, Poll};

/// Crate-private alias so `src/url.rs` — copied byte-for-byte from
/// `plugins/url-launcher/src/url.rs` (see this module's own
/// `url_rs_stays_byte_identical_to_url_launcher` tripwire test below) —
/// compiles unchanged against this crate's own error enum. `url.rs`
/// references `crate::UrlLauncherError` and constructs exactly four of its
/// variants (`InvalidUrl`, `PlatformNotInitialized`, `NoHandler`,
/// `Platform(String)`); every one of those also exists on
/// [`AuthSessionError`], so this alias — not a fork of the file — is the
/// only accommodation this crate makes to reuse it.
type UrlLauncherError = AuthSessionError;

// rustdoc cannot always resolve an enum-variant intra-doc link
// (`crate::UrlLauncherError::InvalidUrl`, `url.rs`'s own module doc) through
// a type alias to a differently-named enum. The allow lives on the `mod url`
// declaration, not inside `url.rs` itself, so that file stays byte-identical
// to `plugins/url-launcher/src/url.rs`.
#[allow(rustdoc::broken_intra_doc_links)]
mod url;

mod oneshot;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_vendor = "apple")]
mod apple;
#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
mod unsupported;

#[cfg(target_os = "android")]
use android as backend;
#[cfg(target_vendor = "apple")]
use apple as backend;
#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
use unsupported as backend;

/// A request to start an [`AuthSession`] — the identity provider's
/// authorization URL plus the custom scheme it will redirect back to.
///
/// `Debug`-formats without ever printing [`Self::url`] (this crate doc's
/// *The URL never appears in a `Display`/`Debug` string* section): a hand
/// implementation prints only the URL's byte length and the (non-secret)
/// callback scheme.
#[derive(Clone)]
pub struct AuthSessionRequest {
    /// The identity provider's authorization endpoint — must be an absolute
    /// `https` URL (see [`AuthSession::start`]'s validation).
    pub url: String,
    /// The custom URL scheme the identity provider redirects back to on
    /// completion (e.g. `myapp`, `com.example.app`) — validated against
    /// [`AuthSession::start`]'s scheme rule table.
    pub callback_scheme: String,
    /// Request a private/ephemeral browsing session with no persisted
    /// cookies or history (`ASWebAuthenticationSession.prefersEphemeralWebBrowserSession`
    /// on Apple; Android has no first-class equivalent and this crate's
    /// stub Android backend ignores it — the real Android backend card
    /// documents its own best-effort mapping, if any).
    pub ephemeral: bool,
}

impl fmt::Debug for AuthSessionRequest {
    /// Never prints [`Self::url`] (an authorization URL can carry
    /// provider-specific session state) — only its byte length and the
    /// callback scheme, which is not secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthSessionRequest")
            .field("url_len", &self.url.len())
            .field("callback_scheme", &self.callback_scheme)
            .field("ephemeral", &self.ephemeral)
            .finish()
    }
}

/// How an [`AuthSession`] finished.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthSessionOutcome {
    /// The identity provider redirected back to `callback_scheme://…` —
    /// the full callback URL, including its query string (the authorization
    /// code and any `state`).
    Callback(String),
    /// The user dismissed the in-app browser tab before the identity
    /// provider ever redirected.
    Cancelled,
}

/// Errors from an [`AuthSession::start`] call.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant rather than only displaying it. None of these `Display` messages
/// ever echo a URL back (this crate doc's *The URL never appears in a
/// `Display`/`Debug` string* section).
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthSessionError {
    /// [`AuthSessionRequest::url`] is not an absolute `https` URL (the `url`
    /// module's `validate` rule table, plus this crate's own `https`-only
    /// narrowing of it), or [`AuthSessionRequest::callback_scheme`] fails
    /// this crate's scheme rule table (1-64 bytes, lowercase-alpha first
    /// byte, lowercase-alnum/`+`/`-`/`.` thereafter, not one of the denied
    /// well-known schemes).
    #[error(
        "auth session: request URL is not an absolute https URL or the callback scheme is invalid"
    )]
    InvalidUrl,

    /// Another [`AuthSession`] is already live — at most one runs at a time,
    /// process-wide (this crate doc's *Exactly one live session* section).
    #[error("auth session: another session is already live")]
    Busy,

    /// The Android host shell never installed the `(JavaVM, Context)`
    /// platform handles this crate's Android backend needs
    /// (`frust-plugin`'s pre-init state) — an old scaffold predating
    /// `nativeInitPlatform`. Never a panic; the caller degrades gracefully.
    #[error("auth session platform not initialized")]
    PlatformNotInitialized,

    /// No platform authentication user agent exists on this target (see
    /// `unsupported`'s module doc for the recommended fallback).
    #[error("auth session: no authentication user agent on this platform")]
    NoHandler,

    /// A backend-specific failure that isn't a not-yet-initialized/
    /// no-handler condition — a platform JNI/Objective-C error this crate
    /// doesn't otherwise classify. Also covers a backend module dropping its
    /// resolution handle without ever calling back (this crate's `oneshot`
    /// module's drop-without-send fallback), and this crate's own stub
    /// `android`/`apple` backends (`"backend not implemented"`) until
    /// the platform backend cards replace them.
    #[error("auth session error: {0}")]
    Platform(String),
}

/// A token handed to a backend's `start`, naming the one live session
/// [`ACTIVE`] is tracking. Carries no data of its own: a backend calls
/// [`resolve`] (which looks the live `oneshot::Sender` up from [`ACTIVE`]
/// directly) rather than holding a channel handle of its own, since this
/// crate's single-active-session invariant (guarded by
/// [`AuthSessionError::Busy`]) means at most one session — and therefore at
/// most one meaningful [`resolve`] call — is ever live at a time. A real
/// backend (the platform cards) moves this token into whatever
/// completion-block/JNI-callback closure eventually calls [`resolve`], so
/// the closure's shape documents which session it belongs to even though
/// nothing about resolution itself reads the token's contents.
pub(crate) struct SessionToken(());

/// The one live session's resolution channel, if any — the process-wide
/// Busy guard (this crate doc's *Exactly one live session* section).
static ACTIVE: Mutex<Option<oneshot::Sender>> = Mutex::new(None);

/// Lock [`ACTIVE`], recovering from poisoning instead of panicking — a panic
/// on one side (the polling caller or a platform backend callback) must not
/// turn the other side's next call into a panic too
/// (`docs/CODE_STANDARDS.md`'s no-panic rule near an FFI boundary; matches
/// `plugins/camera/src/android.rs`'s own `lock` helper). The guarded data is
/// a plain `Option`, so a poisoned view is still coherent enough to use.
fn lock_active() -> MutexGuard<'static, Option<oneshot::Sender>> {
    ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Complete the one live session with `outcome`, releasing [`ACTIVE`]'s
/// slot. The backends call this exactly once per session, from whatever
/// thread the platform's completion callback lands on. A call with no live
/// session (this crate's own error path already resolved and released it,
/// or a stray/duplicate callback) is a no-op — never a panic.
///
/// Unused on every target this crate's originating task builds against: its
/// stub `android`/`apple` backends reject synchronously from `start`
/// itself (handled by [`StartFuture::new`]'s own error path, which resolves
/// through a [`oneshot::Sender`] directly rather than this function), and
/// [`unsupported::start`] never presents a session to resolve at all. This
/// is the entry point a real, asynchronous backend's completion callback
/// (a JNI callback, an Objective-C completion block dispatched later) calls
/// once it actually has an outcome — the Android/Apple backend cards are the
/// first real callers.
#[allow(dead_code)]
pub(crate) fn resolve(outcome: Result<AuthSessionOutcome, AuthSessionError>) {
    match lock_active().take() {
        Some(sender) => sender.send(outcome),
        None => {
            // No `log` dependency in this crate's Cargo.toml (the
            // platform-plugin charter keeps this crate's dependency list to
            // exactly what its originating task named), so this is a
            // debug-only trace rather than a real logging call.
            #[cfg(debug_assertions)]
            eprintln!("frust-auth-session: resolve() called with no live session");
        }
    }
}

/// Validate `req` against this crate's URL and callback-scheme rule tables.
/// `Ok(())` means `req` is safe to hand to a backend.
fn validate(req: &AuthSessionRequest) -> Result<(), AuthSessionError> {
    url::validate(&req.url)?;

    // `url::validate` (the `url` module's rule table) accepts `http` or
    // `https`; RFC 8252's in-app browser tab additionally requires `https`
    // specifically (an OAuth authorization endpoint is never plain `http`).
    let colon = req
        .url
        .find(':')
        .expect("url::validate already proved a scheme is present");
    if !req.url[..colon].eq_ignore_ascii_case("https") {
        return Err(AuthSessionError::InvalidUrl);
    }

    validate_callback_scheme(&req.callback_scheme)
}

/// Well-known schemes a callback must not claim — each already has its own
/// platform meaning ([`url`]'s own `http`/`https` gate, a script/data/file
/// injection vector, or a scheme a mobile OS treats specially), so accepting
/// any of them as a *custom* auth-session callback scheme would let a
/// redirect masquerade as one of those instead.
const DENIED_CALLBACK_SCHEMES: [&str; 9] = [
    "http",
    "https",
    "file",
    "javascript",
    "data",
    "blob",
    "intent",
    "content",
    "about",
];

/// Validate a callback scheme: 1-64 bytes, a lowercase ASCII-alpha first
/// byte, lowercase ASCII alphanumeric/`+`/`-`/`.` thereafter (RFC 3986's
/// `scheme` grammar narrowed to lowercase-only, since two apps registering
/// the same scheme in different cases would otherwise collide unpredictably
/// on some platforms), and not one of [`DENIED_CALLBACK_SCHEMES`].
fn validate_callback_scheme(scheme: &str) -> Result<(), AuthSessionError> {
    let bytes = scheme.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return Err(AuthSessionError::InvalidUrl);
    }
    if !bytes[0].is_ascii_lowercase() {
        return Err(AuthSessionError::InvalidUrl);
    }
    if !bytes[1..].iter().all(|&byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.')
    }) {
        return Err(AuthSessionError::InvalidUrl);
    }
    if DENIED_CALLBACK_SCHEMES.contains(&scheme) {
        return Err(AuthSessionError::InvalidUrl);
    }
    Ok(())
}

/// The future [`AuthSession::start`] returns — either an already-resolved
/// value (a validation failure, [`AuthSessionError::Busy`], or a backend's
/// synchronous pre-dispatch failure) or the live [`oneshot::Receiver`] a
/// backend resolves later. A single concrete type covers both cases so
/// [`AuthSession::start`]'s `-> impl Future` return type is one type
/// regardless of which path a given call takes.
enum StartFuture {
    Ready(Option<Result<AuthSessionOutcome, AuthSessionError>>),
    Pending(oneshot::Receiver),
}

impl StartFuture {
    fn new(req: AuthSessionRequest) -> Self {
        if let Err(err) = validate(&req) {
            return StartFuture::Ready(Some(Err(err)));
        }

        let mut active = lock_active();
        if active.is_some() {
            return StartFuture::Ready(Some(Err(AuthSessionError::Busy)));
        }

        let (sender, receiver) = oneshot::channel();
        *active = Some(sender);
        // Release the lock before calling into the backend: a backend that
        // resolves synchronously (this crate's stub `android`/`apple`
        // modules do not, but a future real backend's early-failure path
        // might, via `resolve`) must not deadlock on a lock this function
        // itself is still holding.
        drop(active);

        match backend::start(req, SessionToken(())) {
            Ok(()) => StartFuture::Pending(receiver),
            Err(err) => {
                // The backend rejected the request before ever presenting a
                // session UI (e.g. this crate's stub backends, or a real
                // backend's `PlatformNotInitialized` check) — release the
                // slot and resolve the receiver with that error, rather than
                // leaving it live for nothing to ever complete.
                if let Some(sender) = lock_active().take() {
                    sender.send(Err(err));
                }
                StartFuture::Pending(receiver)
            }
        }
    }
}

impl Future for StartFuture {
    type Output = Result<AuthSessionOutcome, AuthSessionError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            StartFuture::Ready(value) => Poll::Ready(
                value
                    .take()
                    .expect("StartFuture::Ready polled after completion"),
            ),
            StartFuture::Pending(receiver) => Pin::new(receiver).poll(cx),
        }
    }
}

/// The in-app-browser authentication-session entry point.
///
/// Carries no state — [`Self::start`] is a plain associated function,
/// matching `frust-haptics`'s `Haptics`/`frust-url-launcher`'s
/// `UrlLauncher`.
pub struct AuthSession;

impl AuthSession {
    /// Start an in-app-browser authentication session for `req`, returning a
    /// future that resolves once the identity provider redirects back (or
    /// the user cancels) — see this crate doc's *The awaitable-future
    /// contract* section for how to poll it.
    ///
    /// # Errors
    /// [`AuthSessionError::InvalidUrl`] if [`AuthSessionRequest::url`] is not
    /// an absolute `https` URL or
    /// [`AuthSessionRequest::callback_scheme`] fails this crate's scheme
    /// rule table — checked before any platform API is touched, and
    /// resolved on the future's first poll rather than deferred.
    /// [`AuthSessionError::Busy`] if another [`AuthSession`] is already
    /// live, also resolved on first poll. Otherwise, per platform:
    /// [`AuthSessionError::PlatformNotInitialized`]/
    /// [`AuthSessionError::NoHandler`]/[`AuthSessionError::Platform`] for a
    /// backend-specific pre-dispatch failure (resolved on first poll), or —
    /// once a session is genuinely presented — [`AuthSessionOutcome`] or a
    /// later [`AuthSessionError::Platform`] once the platform actually
    /// responds.
    pub fn start(
        req: AuthSessionRequest,
    ) -> impl Future<Output = Result<AuthSessionOutcome, AuthSessionError>> + Send + 'static {
        StartFuture::new(req)
    }

    /// Whether this build target has a real (non-stub, non-`unsupported`)
    /// authentication-session backend at all — Android or Apple
    /// (`target_vendor = "apple"`, so iOS and macOS both). Does not
    /// distinguish this crate's current stub `android`/`apple` backends
    /// from a future real implementation; both report `true` here, since
    /// both compile the same backend module in.
    pub fn is_supported() -> bool {
        cfg!(any(target_os = "android", target_vendor = "apple"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::task::{RawWaker, RawWakerVTable, Waker};

    use super::*;

    /// Serializes every test in this module — [`ACTIVE`] is one
    /// process-global static, so two tests racing on it (Rust runs
    /// `#[test]`s on separate threads within one binary by default) would
    /// flake into each other's `Busy`/`NoHandler` outcomes. Matches
    /// `plugins/iap/src/event.rs`'s own `TEST_LOCK` precedent for the same
    /// reason.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    /// Run `f` with the `TEST_LOCK` held and [`ACTIVE`] guaranteed empty
    /// both before and after — a test that leaves a stray session live
    /// would otherwise poison every later test with a spurious `Busy`.
    fn with_clean_active<T>(f: impl FnOnce() -> T) -> T {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lock_active().take();
        let result = f();
        lock_active().take();
        result
    }

    fn valid_request() -> AuthSessionRequest {
        AuthSessionRequest {
            url: "https://issuer.example/authorize".to_string(),
            callback_scheme: "frustplay".to_string(),
            ephemeral: false,
        }
    }

    /// A `Waker` that does nothing — sufficient for these tests, which only
    /// ever poll a future that resolves immediately (this crate's stub
    /// backends never leave a session genuinely pending).
    fn noop_waker() -> Waker {
        fn clone(_data: *const ()) -> RawWaker {
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        fn noop(_data: *const ()) {}
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    fn poll_once(
        fut: &mut (impl Future<Output = Result<AuthSessionOutcome, AuthSessionError>> + Unpin),
    ) -> Poll<Result<AuthSessionOutcome, AuthSessionError>> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        Pin::new(fut).poll(&mut cx)
    }

    #[test]
    fn rejects_non_https_scheme() {
        with_clean_active(|| {
            let mut req = valid_request();
            req.url = "http://issuer.example/authorize".to_string();
            let mut fut = AuthSession::start(req);
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Err(AuthSessionError::InvalidUrl))
            );
        });
    }

    #[test]
    fn callback_scheme_rule_table() {
        for scheme in ["loop", "frustplay", "com.example.app"] {
            assert!(
                validate_callback_scheme(scheme).is_ok(),
                "{scheme:?} should be accepted"
            );
        }
        let sixty_five_bytes = "a".repeat(65);
        for scheme in [
            "Loop",
            "1abc",
            "",
            "https",
            "javascript",
            sixty_five_bytes.as_str(),
            "a b",
        ] {
            assert!(
                validate_callback_scheme(scheme).is_err(),
                "{scheme:?} should be rejected"
            );
        }
    }

    #[test]
    fn unsupported_or_stub_backend_reports_no_handler_or_platform_and_releases_the_slot() {
        with_clean_active(|| {
            let mut fut = AuthSession::start(valid_request());
            let outcome = poll_once(&mut fut);
            // Android/Apple: this crate's stub backends reject with
            // `Platform("backend not implemented")`; every other target:
            // `unsupported::start` rejects with `NoHandler`. Either way the
            // future must resolve on first poll and the slot must be
            // released.
            assert!(
                matches!(
                    outcome,
                    Poll::Ready(Err(AuthSessionError::NoHandler))
                        | Poll::Ready(Err(AuthSessionError::Platform(_)))
                ),
                "expected Ready(Err(NoHandler | Platform(_))), got {outcome:?}"
            );
            assert!(
                lock_active().is_none(),
                "the slot must be released after a pre-dispatch failure"
            );

            // A second start sees the same pre-dispatch failure again, not
            // `Busy` — proof the first call actually released the slot
            // rather than leaking it.
            let mut fut2 = AuthSession::start(valid_request());
            let outcome2 = poll_once(&mut fut2);
            assert!(
                matches!(
                    outcome2,
                    Poll::Ready(Err(AuthSessionError::NoHandler))
                        | Poll::Ready(Err(AuthSessionError::Platform(_)))
                ),
                "expected Ready(Err(NoHandler | Platform(_))) again, got {outcome2:?}"
            );
        });
    }

    #[test]
    fn busy_when_a_session_is_already_live_then_resolve_frees_the_slot() {
        with_clean_active(|| {
            // Simulate an accepted, still-live session by inserting a
            // `Sender` directly — this crate's own stub backends never
            // leave a session live long enough to observe `Busy` for real,
            // so the test hook bypasses backend dispatch entirely (this
            // module's own doc's *Busy* test).
            let (sender, _receiver) = oneshot::channel();
            *lock_active() = Some(sender);

            let mut fut = AuthSession::start(valid_request());
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Err(AuthSessionError::Busy))
            );

            resolve(Ok(AuthSessionOutcome::Cancelled));
            assert!(lock_active().is_none(), "resolve() must release the slot");
        });
    }

    #[test]
    fn resolve_with_no_live_session_is_a_no_op() {
        with_clean_active(|| {
            // No panic, no live session to disturb.
            resolve(Ok(AuthSessionOutcome::Cancelled));
            assert!(lock_active().is_none());
        });
    }

    /// Guards `src/url.rs` staying byte-identical to
    /// `plugins/url-launcher/src/url.rs` — this crate's `UrlLauncherError`
    /// alias (this file's own doc comment above `type UrlLauncherError`)
    /// only compiles because the two files are the same source, so a future
    /// edit to either one that silently forks them would otherwise go
    /// unnoticed until the two crates' validators drifted apart.
    #[test]
    fn url_rs_stays_byte_identical_to_url_launcher() {
        let ours = include_str!("url.rs");
        let theirs = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../url-launcher/src/url.rs"
        ))
        .expect("sibling url-launcher checkout");
        assert_eq!(
            ours, theirs,
            "plugins/auth-session/src/url.rs must stay byte-identical to plugins/url-launcher/src/url.rs"
        );
    }
}
