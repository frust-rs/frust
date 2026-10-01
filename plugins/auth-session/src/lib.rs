//! `frust-auth-session`: an RFC 8252-style "OAuth for native apps" in-app
//! browser session — Android Custom Tabs / macOS+iOS
//! `ASWebAuthenticationSession` presented over the request's `https` URL,
//! resolving to the redirect the identity provider sends back to
//! `callback_scheme://…` (or the user cancelling) — plus, for desktop Linux,
//! Windows and macOS, an RFC 8252 loopback-redirect listener
//! ([`LoopbackSession`]) that opens the system browser and receives the
//! redirect on `http://127.0.0.1:<port>/<path>` (*Desktop loopback* below).
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
//! UI surface on every backend platform). The busy slot is **shared across
//! both backends**: a bound [`LoopbackSession`] holds the same slot from
//! [`LoopbackSession::bind`] until it resolves or is dropped, so while one is
//! live both [`AuthSession::start`] and a second [`LoopbackSession::bind`]
//! answer [`AuthSessionError::Busy`], and vice versa. Each accepted `start` is stamped
//! with a fresh, process-wide **generation**, and a platform result completes
//! a session only while that session's generation is still the live one — a
//! result reported after the session finished or was abandoned is discarded
//! rather than resolving whatever session happens to be live now. The
//! generation is bookkeeping on this side of the platform boundary, not
//! evidence about the redirect: on Android the host stamps whichever
//! callback-scheme `Intent` it observes with the generation pending at that
//! moment (the `android` module's *The generation round trip*), so a stale
//! tab's late redirect can still complete the live session. PKCE and `state`
//! (*Security* below) are what bind a callback to a request.
//!
//! The slot releases when the live session resolves (by outcome or by
//! error), when the backend drops its resolution handle without ever
//! calling back (this crate's `oneshot` module's own drop-without-send
//! fallback), **or** when the caller drops the future
//! [`AuthSession::start`] returned. That last one is the only bound on a
//! wedged custom-scheme session: [`AuthSession`] runs no timeout of its own
//! ([`LoopbackSession`] does — [`LoopbackOptions::timeout`]), so a platform
//! that presents a session and then never calls back keeps the slot claimed
//! for exactly as long as the caller holds that future alive — dropping it
//! (or the task awaiting it) frees the slot for the next `start`.
//!
//! Releasing the slot is **not** the same as ending the platform UI. A
//! dropped future only frees this crate's bookkeeping: on Android the Custom
//! Tab stays on screen until the user closes it (a redirect it delivers
//! afterwards finds no pending session and is dropped); on iOS/macOS the
//! presented `ASWebAuthenticationSession` sheet stays up until the user
//! dismisses it or the next [`AuthSession::start`] cancels it in favour of
//! the new session. There is no cancel API in v1 (`auth-session-no-cancel-v1`
//! in `docs/LIMITATIONS.md`).
//!
//! # Security
//!
//! **Callers MUST use PKCE (RFC 7636, `S256`) and MUST verify the `state`
//! parameter they generated before exchanging the authorization code.** No
//! platform can prove a callback came from the session it launched: on
//! Android the redirect arrives as an ordinary `Intent` that any app
//! registering the same custom scheme can deliver, and while iOS/macOS
//! intercept the redirect in-process inside the session's own browser tab,
//! the redirect itself is still an unauthenticated HTTP response. An
//! [`AuthSessionOutcome::Callback`] is therefore evidence that *a* redirect
//! to the requested scheme arrived — never that the identity provider this
//! session presented is the party that sent it. PKCE binds the code to the
//! code verifier this app generated, and the `state` check binds the
//! callback to the request this app made; without both, a callback that
//! reaches this future is enough to complete someone else's flow.
//!
//! # Desktop loopback
//!
//! [`LoopbackSession`] is RFC 8252 §7.3's loopback interface redirection,
//! for targets with no in-app browser tab (desktop Linux and Windows) and
//! for macOS apps that prefer the system browser:
//!
//! 1. [`LoopbackSession::bind`] validates [`LoopbackOptions`] (a reserved
//!    path of 2-256 bytes of ASCII alphanumerics and `-._~/`, starting with
//!    `/`, with no `//`, `.` or `..` segment and no `?`, `#` or `%`; a
//!    timeout above zero and at most one hour — else
//!    [`AuthSessionError::InvalidUrl`]), answers
//!    [`AuthSessionError::NoHandler`] off desktop without opening a socket,
//!    claims the shared busy slot (*Exactly one live session*), and binds the
//!    literal `127.0.0.1` on an ephemeral port — never `localhost`,
//!    `0.0.0.0` or `::`. [`LoopbackSession::redirect_uri`] is then exactly
//!    `http://127.0.0.1:<port><path>`.
//! 2. [`LoopbackSession::start`] checks the authorization URL with the same
//!    `https`-only rule as [`AuthSession::start`], spawns one thread that
//!    owns the listener, and opens the URL in the system browser via
//!    `frust-url-launcher`. The future resolves to the full callback URL
//!    (query intact byte-for-byte), [`AuthSessionOutcome::Cancelled`] after
//!    [`LoopbackCancel::cancel`], [`AuthSessionError::TimedOut`], or the
//!    browser launch's own error. Dropping it stops the listener.
//! 3. The listener polls every 25 ms and closes its socket on every exit
//!    path; it accepts exactly one callback, then stops.
//! 4. Connections are served one at a time, each with a 2 s / 8 KiB budget
//!    for its request head. A request must be a well-formed
//!    `HTTP/1.0`/`HTTP/1.1` request line with a printable-ASCII,
//!    `#`-free target and exactly one `Host: 127.0.0.1:<port>` header (the
//!    DNS-rebinding defence) — else `400`; a non-`GET` method or a path
//!    other than the reserved one is `404`. After a `400`/`404` the listener
//!    keeps waiting, so a stray local client cannot end the session.
//! 5. Every response is a static page with `Cache-Control: no-store`,
//!    `Content-Security-Policy: default-src 'none'`, `Referrer-Policy:
//!    no-referrer`, `X-Content-Type-Options: nosniff` and `Connection:
//!    close`; nothing from the request is ever echoed, and the success page
//!    carries no script.
//! 6. No `Debug`/`Display` output or error string names the callback URL or
//!    its query; [`LoopbackSession`]'s `Debug` prints only port and path.
//!
//! Any local process can reach the port while it is open, so *Security*'s
//! PKCE + `state` requirement applies unchanged.
//!
//! # Platform notes
//!
//! | Target | [`AuthSession`] (custom scheme) | [`LoopbackSession`] |
//! |---|---|---|
//! | Android | Chrome Custom Tabs (`android` module) | [`AuthSessionError::NoHandler`] |
//! | iOS | `ASWebAuthenticationSession` (`apple` module) | [`AuthSessionError::NoHandler`] |
//! | macOS | `ASWebAuthenticationSession` (`apple` module) | system browser + `127.0.0.1` listener |
//! | Linux, Windows | [`AuthSessionError::NoHandler`] (`unsupported` module) | system browser + `127.0.0.1` listener |
//! | anything else | [`AuthSessionError::NoHandler`] | [`AuthSessionError::NoHandler`] |
//!
//! [`AuthSession::is_supported`] is `true` on the first four rows — on Linux
//! and Windows that means the [`LoopbackSession`] path.
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
//! Neither [`AuthSessionRequest`]'s nor [`AuthSessionOutcome`]'s
//! hand-written [`core::fmt::Debug`] impl, nor any [`AuthSessionError`]
//! variant's `Display` message, ever echoes `req.url` or the resolved
//! callback URL back into a log line — an
//! authorization code or an identity-provider session detail can ride along
//! in either one, so this crate never interpolates a URL into anything that
//! might get logged (see [`AuthSessionRequest`]'s own doc and this crate's
//! `url` module's copied-from-`frust-url-launcher` test of the same rule).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
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

mod loopback;
pub use loopback::{DEFAULT_LOOPBACK_TIMEOUT, LoopbackCancel, LoopbackOptions, LoopbackSession};

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
    /// cookies or history
    /// (`ASWebAuthenticationSession.prefersEphemeralWebBrowserSession` on
    /// Apple; `CustomTabsIntent.Builder#setEphemeralBrowsingEnabled` on
    /// Android, where it is **advisory** — a browser that does not
    /// implement it silently runs an ordinary session instead).
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
///
/// `Debug`-formats without ever printing the callback URL (this crate doc's
/// *The URL never appears in a `Display`/`Debug` string* section): the
/// hand-written impl below prints `Callback { url_len: N }` rather than the
/// URL itself. Read the URL by matching the value — `Callback(url)` — never
/// out of a `Debug` rendering.
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq)]
pub enum AuthSessionOutcome {
    /// The identity provider redirected back to `callback_scheme://…` —
    /// the full callback URL, including its query string (the authorization
    /// code and any `state`).
    Callback(String),
    /// The user dismissed the in-app browser tab before the identity
    /// provider ever redirected.
    Cancelled,
}

impl fmt::Debug for AuthSessionOutcome {
    /// Never prints the callback URL — an authorization code and the
    /// provider's `state` both ride in it — only its byte length.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Callback(url) => f
                .debug_struct("Callback")
                .field("url_len", &url.len())
                .finish(),
            Self::Cancelled => f.write_str("Cancelled"),
        }
    }
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
    /// `unsupported`'s module doc — on desktop Linux and Windows, use
    /// [`LoopbackSession`]), or [`LoopbackSession`]'s browser launch found no
    /// handler for `https` URLs.
    #[error("auth session: no authentication user agent on this platform")]
    NoHandler,

    /// A started [`LoopbackSession`] received no matching redirect within its
    /// [`LoopbackOptions::timeout`]; the listener has been closed.
    #[error("auth session: timed out waiting for the authorization redirect")]
    TimedOut,

    /// A backend-specific failure that isn't a not-yet-initialized/
    /// no-handler condition — a platform JNI/Objective-C error this crate
    /// doesn't otherwise classify. Also covers a backend module dropping its
    /// resolution handle without ever calling back (this crate's `oneshot`
    /// module's drop-without-send fallback) and a callback this crate
    /// refuses to hand back: one delivered with no URL at all, or one whose
    /// scheme is not the [`AuthSessionRequest::callback_scheme`] the
    /// session asked for.
    #[error("auth session error: {0}")]
    Platform(String),
}

/// The [`AuthSessionError::Platform`] message for a `Callback` a backend
/// delivered with no URL in it. A crate-level constant because the
/// `android` backend rejects the same condition one layer earlier (a null
/// or empty JNI string), and the two must read identically.
pub(crate) const CALLBACK_WITHOUT_URL: &str = "callback delivered without a URL";

/// The [`AuthSessionError::Platform`] message for a callback URL whose
/// scheme is not the one the session requested. Never names either scheme
/// or the URL (this crate doc's URL rule — the URL is the secret-bearing
/// half, and a message naming only one of the two invites pasting the other
/// one in later).
const CALLBACK_SCHEME_MISMATCH: &str =
    "callback URL scheme does not match the requested callback scheme";

/// A token handed to a backend's `start`, naming the one session that call
/// is presenting. A backend carries [`Self::generation`] through whatever
/// completion-block/JNI-callback path eventually produces an outcome and
/// hands it back to [`resolve`], which is what makes every resolution
/// attributable to the session that produced it: a result stamped with a
/// generation other than the live one is discarded rather than completing
/// whichever session happens to be live by then.
pub(crate) struct SessionToken {
    /// This session's generation — see [`next_generation`].
    ///
    /// Read by the `android`/`apple` backends only. The `unsupported`
    /// backend refuses every request without ever presenting a session, so
    /// on those targets the field is constructed and dropped unread; that
    /// is the whole of this `allow`, and it is deliberately not applied on
    /// a target whose backend must use it.
    #[cfg_attr(
        not(any(target_os = "android", target_vendor = "apple")),
        allow(dead_code)
    )]
    pub(crate) generation: u64,
}

/// The one live session, if any — the process-wide Busy guard (this crate
/// doc's *Exactly one live session* section) plus everything resolution
/// needs to attribute and check an incoming platform result.
struct Live {
    /// The generation stamped on the [`SessionToken`] this session's
    /// backend was started with; only a result carrying it may complete
    /// this session.
    generation: u64,
    /// The [`AuthSessionRequest::callback_scheme`] this session asked for,
    /// kept so [`resolve`] can re-check the scheme of a delivered callback
    /// URL against it rather than trusting the platform to have filtered.
    callback_scheme: String,
    /// The channel half that completes the caller's future.
    sender: oneshot::Sender,
}

/// The one live session's slot.
static ACTIVE: Mutex<Option<Live>> = Mutex::new(None);

/// The generation counter behind [`next_generation`]. Starts at `1` so a
/// generation is never `0` — a zero-initialized `jlong`/`u64` reaching
/// [`resolve`] from a host that lost track of its own session must not look
/// like a valid one.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// A fresh, process-wide generation for one accepted session. Never `0`,
/// including across the (theoretical) wrap of [`NEXT_GENERATION`].
fn next_generation() -> u64 {
    loop {
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        if generation != 0 {
            return generation;
        }
    }
}

/// Lock [`ACTIVE`], recovering from poisoning instead of panicking — a panic
/// on one side (the polling caller or a platform backend callback) must not
/// turn the other side's next call into a panic too
/// (`docs/CODE_STANDARDS.md`'s no-panic rule near an FFI boundary; matches
/// `plugins/camera/src/android.rs`'s own `lock` helper). The guarded data is
/// a plain `Option`, so a poisoned view is still coherent enough to use.
fn lock_active() -> MutexGuard<'static, Option<Live>> {
    ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Take the live session out of [`ACTIVE`], but only if it is still
/// generation `generation`'s. Returns with the lock already released, so a
/// caller is free to complete (and therefore wake) the returned sender.
fn take_live(generation: u64) -> Option<Live> {
    let mut active = lock_active();
    if active
        .as_ref()
        .is_some_and(|live| live.generation == generation)
    {
        active.take()
    } else {
        None
    }
}

/// Release generation `generation`'s slot without completing anything —
/// the hook `oneshot::Receiver`'s `Drop` calls when the caller drops the
/// future [`AuthSession::start`] returned. A later platform result for that
/// generation then finds no live session and is discarded by [`resolve`]'s
/// own generation check, so an abandoned session can neither wedge the slot
/// nor resolve a session started after it.
pub(crate) fn release_if_live(generation: u64) {
    // `take_live` has already released the lock by the time the `Live` is
    // dropped here — dropping it drops its `Sender`, whose own drop wakes
    // the channel, which may run arbitrary caller code.
    drop(take_live(generation));
}

/// Complete generation `generation`'s session with `outcome`, releasing
/// [`ACTIVE`]'s slot. The backends call this from whatever thread the
/// platform's completion callback lands on, and this crate's own
/// pre-dispatch error path calls it too.
///
/// A result whose generation is not the live one — a late callback from a
/// session the caller already abandoned, or a duplicate delivery — leaves
/// [`ACTIVE`] untouched and is discarded. So is a result arriving with no
/// live session at all. Never a panic, on any path.
///
/// An `Ok(`[`AuthSessionOutcome::Callback`]`)` is additionally re-checked
/// against the live session's own callback scheme before it completes
/// anything — see [`checked_callback`].
pub(crate) fn resolve(generation: u64, outcome: Result<AuthSessionOutcome, AuthSessionError>) {
    let Some(live) = take_live(generation) else {
        // No `log` dependency in this crate's Cargo.toml (the
        // platform-plugin charter — `docs/PLUGINS_CODE_STANDARDS.md` —
        // keeps this crate to `frust-plugin` plus FFI crates), so this is
        // a debug-only trace rather than a real logging call.
        #[cfg(debug_assertions)]
        eprintln!("frust-auth-session: stale session result discarded");
        return;
    };

    // The lock is already released: `Sender::send` wakes the caller's
    // waker, which may run arbitrary caller code — including a `start` of
    // the next session — so `ACTIVE` must never be held across it.
    let outcome = checked_callback(&live.callback_scheme, outcome);
    live.sender.send(outcome);
}

/// Re-check a delivered callback URL against `callback_scheme`, the scheme
/// the live session actually asked for. Anything that is not an
/// `Ok(`[`AuthSessionOutcome::Callback`]`)` passes through untouched.
///
/// Two rejections, both [`AuthSessionError::Platform`] and neither naming
/// the URL (this crate doc's URL rule):
///
/// - an empty URL — a host that reported a callback but carried nothing in
///   it, which a caller would otherwise parse as a real answer;
/// - a URL whose scheme (the bytes before its first `:`, compared
///   ASCII-case-insensitively — [`validate_callback_scheme`] already
///   restricts the requested scheme to lowercase) is not the requested one,
///   or which has no scheme at all.
///
/// This is a consistency check on the platform's own delivery, **not** an
/// authenticity check: a matching scheme proves nothing about who sent the
/// redirect (this crate doc's *Security* section).
fn checked_callback(
    callback_scheme: &str,
    outcome: Result<AuthSessionOutcome, AuthSessionError>,
) -> Result<AuthSessionOutcome, AuthSessionError> {
    let Ok(AuthSessionOutcome::Callback(url)) = &outcome else {
        return outcome;
    };
    if url.is_empty() {
        return Err(AuthSessionError::Platform(CALLBACK_WITHOUT_URL.to_string()));
    }
    match url.find(':') {
        Some(colon) if url[..colon].eq_ignore_ascii_case(callback_scheme) => outcome,
        _ => Err(AuthSessionError::Platform(
            CALLBACK_SCHEME_MISMATCH.to_string(),
        )),
    }
}

/// Validate `req` against this crate's URL and callback-scheme rule tables.
/// `Ok(())` means `req` is safe to hand to a backend.
fn validate(req: &AuthSessionRequest) -> Result<(), AuthSessionError> {
    validate_https_url(&req.url)?;
    validate_callback_scheme(&req.callback_scheme)
}

/// Validate an authorization URL: the `url` module's rule table, narrowed to
/// `https`. Shared by [`AuthSession::start`] (through [`validate`]) and
/// [`LoopbackSession::start`].
fn validate_https_url(url: &str) -> Result<(), AuthSessionError> {
    url::validate(url)?;

    // `url::validate` (the `url` module's rule table) accepts `http` or
    // `https`; RFC 8252's in-app browser tab additionally requires `https`
    // specifically (an OAuth authorization endpoint is never plain `http`).
    // `url::validate` already proved a scheme is present, so the `else` arm
    // is unreachable; it answers `InvalidUrl` rather than panicking.
    let Some(colon) = url.find(':') else {
        return Err(AuthSessionError::InvalidUrl);
    };
    if !url[..colon].eq_ignore_ascii_case("https") {
        return Err(AuthSessionError::InvalidUrl);
    }
    Ok(())
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
///
/// **After completion, every further poll returns [`Poll::Pending`] on
/// both variants** — the ordinary "do not poll a future after it
/// completed" contract, answered with a permanent `Pending` rather than a
/// panic, because a completion here can be driven by a platform callback on
/// a thread the polling executor knows nothing about (this crate doc's
/// *The awaitable-future contract*) and a spurious re-poll must not be able
/// to take a process down through an FFI callback.
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

        let generation = next_generation();
        let (sender, receiver) = oneshot::channel(generation);
        *active = Some(Live {
            generation,
            callback_scheme: req.callback_scheme.clone(),
            sender,
        });
        // Release the lock before calling into the backend: a backend whose
        // early-failure path resolves synchronously (via `resolve`) must not
        // deadlock on a lock this function itself is still holding.
        drop(active);

        match backend::start(req, SessionToken { generation }) {
            Ok(()) => StartFuture::Pending(receiver),
            Err(err) => {
                // The backend rejected the request before ever presenting a
                // session UI (`unsupported`'s blanket refusal, or a real
                // backend's `PlatformNotInitialized` check) — release the
                // slot and resolve the receiver with that error, rather than
                // leaving it live for nothing to ever complete.
                resolve(generation, Err(err));
                StartFuture::Pending(receiver)
            }
        }
    }
}

impl Future for StartFuture {
    type Output = Result<AuthSessionOutcome, AuthSessionError>;

    /// Polls `Pending` forever once it has completed — see [`StartFuture`]'s
    /// own doc. The `Pending` variant needs no code for that: a
    /// [`oneshot::Receiver`] whose value has already been taken registers
    /// the waker and reports `Pending`, exactly like one still waiting.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            StartFuture::Ready(value) => match value.take() {
                Some(value) => Poll::Ready(value),
                None => Poll::Pending,
            },
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

    /// Whether this build target can run an authentication session at all —
    /// Android (Chrome Custom Tabs), Apple (`target_vendor = "apple"`, so iOS
    /// and macOS both, on `ASWebAuthenticationSession`), or desktop Linux
    /// and Windows. On **Linux and Windows "supported" means the
    /// [`LoopbackSession`] path**: those targets have no in-app browser tab,
    /// so [`Self::start`] (a custom-scheme callback) still answers
    /// [`AuthSessionError::NoHandler`] there (the crate doc's *Platform
    /// notes* table).
    ///
    /// A compile-time answer about the backends this target builds, not a
    /// runtime probe: `true` on Android still leaves
    /// [`AuthSessionError::NoHandler`] reachable at `start` time on a device
    /// with no Custom Tabs-capable browser installed, and `true` on desktop
    /// still leaves it reachable when no system browser is registered.
    pub fn is_supported() -> bool {
        cfg!(any(
            target_os = "android",
            target_vendor = "apple",
            target_os = "linux",
            target_os = "windows"
        ))
    }
}

/// Serializes every test in this crate that touches [`ACTIVE`] — it is one
/// process-global static, so two tests racing on it (Rust runs `#[test]`s on
/// separate threads within one binary by default) would flake into each
/// other's `Busy`/`NoHandler` outcomes. Shared by this file's tests and the
/// `loopback` module's, since both backends claim the same slot. Matches
/// `plugins/iap/src/event.rs`'s own `TEST_LOCK` precedent for the same
/// reason.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use std::task::{RawWaker, RawWakerVTable, Waker};

    use super::*;

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

    /// A `Waker` that does nothing — these tests poll by hand rather than
    /// running an executor, so a wake has nothing to schedule; whether a
    /// value arrived is read from the next [`poll_once`] instead.
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

    /// N4: a reverse-DNS callback scheme — the shape Apple and Android apps
    /// conventionally register — is accepted.
    #[test]
    fn reverse_dns_callback_scheme_is_accepted() {
        assert_eq!(validate_callback_scheme("io.example.console"), Ok(()));
        assert_eq!(validate_callback_scheme("com.example.app"), Ok(()));
    }

    #[test]
    fn is_supported_covers_the_custom_scheme_and_loopback_targets() {
        assert_eq!(
            AuthSession::is_supported(),
            cfg!(any(
                target_os = "android",
                target_vendor = "apple",
                target_os = "linux",
                target_os = "windows"
            ))
        );
        if cfg!(any(target_os = "linux", target_os = "windows")) {
            assert!(AuthSession::is_supported());
            assert!(LoopbackSession::is_supported());
        }
    }

    #[test]
    fn https_url_rule_is_shared_by_both_backends() {
        assert_eq!(
            validate_https_url("https://issuer.example/authorize"),
            Ok(())
        );
        assert_eq!(validate_https_url("HTTPS://issuer.example/a?b=c"), Ok(()));
        for url in [
            "http://issuer.example/",
            "javascript:alert(1)",
            "",
            "https://u@h/",
        ] {
            assert_eq!(
                validate_https_url(url),
                Err(AuthSessionError::InvalidUrl),
                "{url:?}"
            );
        }
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

    /// Claim the `ACTIVE` slot with a live session that no backend is
    /// behind, returning its generation and the future a caller would be
    /// holding. Reaching a genuinely pending session *through* a backend
    /// would need a real Custom Tabs/`ASWebAuthenticationSession` host, so
    /// every resolution test below starts from this hook instead.
    fn live_session(callback_scheme: &str) -> (u64, StartFuture) {
        let generation = next_generation();
        let (sender, receiver) = oneshot::channel(generation);
        *lock_active() = Some(Live {
            generation,
            callback_scheme: callback_scheme.to_string(),
            sender,
        });
        (generation, StartFuture::Pending(receiver))
    }

    /// On a target with no in-app browser tab (desktop Linux and Windows,
    /// where [`LoopbackSession`] is the supported path instead, and every
    /// other non-Android, non-Apple target), the custom-scheme `start`
    /// refuses before presenting anything: the future resolves on its first
    /// poll and the slot is released rather than leaked.
    #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
    #[test]
    fn unsupported_backend_reports_no_handler_on_first_poll_and_releases_the_slot() {
        with_clean_active(|| {
            let mut fut = AuthSession::start(valid_request());
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Err(AuthSessionError::NoHandler))
            );
            assert!(
                lock_active().is_none(),
                "the slot must be released after a pre-dispatch failure"
            );

            // A second start sees the same refusal again, not `Busy` —
            // proof the first call actually released the slot.
            let mut second = AuthSession::start(valid_request());
            assert_eq!(
                poll_once(&mut second),
                Poll::Ready(Err(AuthSessionError::NoHandler))
            );
        });
    }

    /// On Apple, `start` hands the whole present sequence to the main queue
    /// and returns accepted, so the future is genuinely pending and the
    /// slot stays claimed until a completion (or the caller's drop) frees
    /// it. Nothing here drives the main queue: the enqueued block never
    /// runs under a test binary, which is exactly the "claimed, nothing
    /// resolved yet" state being asserted.
    #[cfg(target_vendor = "apple")]
    #[test]
    fn apple_backend_leaves_the_session_pending_with_the_slot_claimed() {
        with_clean_active(|| {
            let mut fut = AuthSession::start(valid_request());
            assert_eq!(poll_once(&mut fut), Poll::Pending);

            let active = lock_active();
            let live = active.as_ref().expect("the slot must stay claimed");
            assert_ne!(live.generation, 0, "a generation is never zero");
            assert_eq!(live.callback_scheme, "frustplay");
        });
    }

    #[test]
    fn busy_when_a_session_is_already_live_then_resolve_frees_the_slot() {
        with_clean_active(|| {
            let (generation, _fut) = live_session("frustplay");

            let mut second = AuthSession::start(valid_request());
            assert_eq!(
                poll_once(&mut second),
                Poll::Ready(Err(AuthSessionError::Busy))
            );

            resolve(generation, Ok(AuthSessionOutcome::Cancelled));
            assert!(lock_active().is_none(), "resolve() must release the slot");
        });
    }

    #[test]
    fn resolve_with_no_live_session_is_a_no_op() {
        with_clean_active(|| {
            // No panic, no live session to disturb.
            resolve(next_generation(), Ok(AuthSessionOutcome::Cancelled));
            assert!(lock_active().is_none());
        });
    }

    #[test]
    fn a_result_carrying_the_live_generation_completes_the_session() {
        with_clean_active(|| {
            let (generation, mut fut) = live_session("frustplay");
            assert_eq!(poll_once(&mut fut), Poll::Pending);

            resolve(
                generation,
                Ok(AuthSessionOutcome::Callback(
                    "frustplay://auth/callback?code=x".to_string(),
                )),
            );
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Ok(AuthSessionOutcome::Callback(
                    "frustplay://auth/callback?code=x".to_string()
                )))
            );
            assert!(lock_active().is_none(), "the slot must be released");
        });
    }

    #[test]
    fn a_result_carrying_a_stale_generation_is_discarded() {
        with_clean_active(|| {
            let (generation, mut fut) = live_session("frustplay");

            // A late callback from an earlier session must neither complete
            // nor release the session that is live now.
            resolve(
                generation.wrapping_sub(1),
                Ok(AuthSessionOutcome::Callback(
                    "frustplay://auth/callback?code=stale".to_string(),
                )),
            );
            assert_eq!(poll_once(&mut fut), Poll::Pending);
            assert!(
                lock_active().is_some(),
                "a stale result must leave the live session untouched"
            );

            // The live generation still completes it afterwards.
            resolve(generation, Ok(AuthSessionOutcome::Cancelled));
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Ok(AuthSessionOutcome::Cancelled))
            );
        });
    }

    #[test]
    fn a_callback_whose_scheme_is_not_the_requested_one_is_refused() {
        with_clean_active(|| {
            let (generation, mut fut) = live_session("frustplay");
            resolve(
                generation,
                Ok(AuthSessionOutcome::Callback(
                    "attacker://auth/callback?code=secret".to_string(),
                )),
            );
            match poll_once(&mut fut) {
                Poll::Ready(Err(AuthSessionError::Platform(message))) => {
                    assert_eq!(
                        message,
                        "callback URL scheme does not match the requested callback scheme"
                    );
                    assert!(
                        !message.contains("secret"),
                        "the message must never quote the URL"
                    );
                }
                other => panic!("expected Ready(Err(Platform(..))), got {other:?}"),
            }
        });
    }

    #[test]
    fn a_callback_scheme_matches_case_insensitively() {
        with_clean_active(|| {
            let (generation, mut fut) = live_session("frustplay");
            resolve(
                generation,
                Ok(AuthSessionOutcome::Callback(
                    "FRUSTPLAY://auth/callback?code=x".to_string(),
                )),
            );
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Ok(AuthSessionOutcome::Callback(
                    "FRUSTPLAY://auth/callback?code=x".to_string()
                )))
            );
        });
    }

    #[test]
    fn a_callback_with_no_url_is_refused() {
        with_clean_active(|| {
            let (generation, mut fut) = live_session("frustplay");
            resolve(generation, Ok(AuthSessionOutcome::Callback(String::new())));
            match poll_once(&mut fut) {
                Poll::Ready(Err(AuthSessionError::Platform(message))) => {
                    assert_eq!(message, "callback delivered without a URL");
                }
                other => panic!("expected Ready(Err(Platform(..))), got {other:?}"),
            }
        });
    }

    #[test]
    fn dropping_the_future_releases_the_slot_for_the_next_session() {
        with_clean_active(|| {
            let (generation, fut) = live_session("frustplay");
            assert!(lock_active().is_some());

            drop(fut);
            assert!(
                lock_active().is_none(),
                "dropping the caller's future must release the slot"
            );

            // The next start is accepted rather than rejected as `Busy` —
            // whatever this target's backend then makes of it.
            let mut next = AuthSession::start(valid_request());
            assert_ne!(
                poll_once(&mut next),
                Poll::Ready(Err(AuthSessionError::Busy))
            );

            // And the abandoned session's own late result is discarded.
            resolve(generation, Ok(AuthSessionOutcome::Cancelled));
        });
    }

    #[test]
    fn a_completed_future_stays_pending_on_every_later_poll() {
        with_clean_active(|| {
            // The `Ready` variant — here a validation failure.
            let mut req = valid_request();
            req.url = "http://issuer.example/authorize".to_string();
            let mut ready = AuthSession::start(req);
            assert_eq!(
                poll_once(&mut ready),
                Poll::Ready(Err(AuthSessionError::InvalidUrl))
            );
            assert_eq!(poll_once(&mut ready), Poll::Pending);
            assert_eq!(poll_once(&mut ready), Poll::Pending);

            // The `Pending` variant — a receiver that already yielded.
            let (generation, mut pending) = live_session("frustplay");
            resolve(generation, Ok(AuthSessionOutcome::Cancelled));
            assert_eq!(
                poll_once(&mut pending),
                Poll::Ready(Ok(AuthSessionOutcome::Cancelled))
            );
            assert_eq!(poll_once(&mut pending), Poll::Pending);
            assert_eq!(poll_once(&mut pending), Poll::Pending);
        });
    }

    /// `AuthSessionOutcome`'s hand-written `Debug` prints the callback
    /// URL's length, never the URL (this crate doc's URL rule) — the shape
    /// a `{:?}` of the whole `Result` ends up carrying.
    #[test]
    fn debug_reports_a_callback_url_by_length_only() {
        let outcome = AuthSessionOutcome::Callback("frustplay://cb?code=secret".to_string());
        let rendered = format!("{outcome:?}");
        assert_eq!(rendered, "Callback { url_len: 26 }");
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("frustplay"));

        assert_eq!(format!("{:?}", AuthSessionOutcome::Cancelled), "Cancelled");
        assert_eq!(
            format!("{:?}", Ok::<_, AuthSessionError>(outcome)),
            "Ok(Callback { url_len: 26 })"
        );
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
