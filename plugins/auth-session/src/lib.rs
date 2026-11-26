//! `frust-auth-session`: an RFC 8252-style "OAuth for native apps" in-app
//! browser session — Android Auth Tab or Custom Tabs / macOS+iOS
//! `ASWebAuthenticationSession` presented over the request's `https` URL,
//! resolving to the redirect the identity provider sends back to the
//! request's [`AuthCallback`] (a custom scheme such as `myapp://…`, or a
//! claimed `https` host and path), or to the user cancelling — plus, for
//! desktop Linux,
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
//! `req.callback`, and resolves with that redirect URL — or
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
//! platform's own main thread (Android's main looper delivering the Auth Tab
//! activity result or the Custom Tabs redirect intent, Apple's completion
//! block dispatched onto the main queue), not by whichever thread happens to be polling, so nothing about
//! this future requires the UI thread specifically — it only *has* to reach
//! the UI thread if the caller's own continuation touches UI state.
//!
//! # Exactly one live session
//!
//! At most one [`AuthSession`] can be in flight at a time, process-wide: a
//! second [`AuthSession::start`] call while one is still live resolves
//! immediately to [`AuthSessionError::Busy`] rather than queuing or
//! replacing the first (an in-app browser tab is a modal, single-instance
//! UI surface on each platform that has one). The busy slot is **shared across
//! both backends**: a bound [`LoopbackSession`] holds the same slot from
//! [`LoopbackSession::bind`] until it resolves or is dropped, so while one is
//! live both [`AuthSession::start`] and a second [`LoopbackSession::bind`]
//! answer [`AuthSessionError::Busy`], and vice versa. Each accepted `start` is stamped
//! with a fresh, process-wide **generation**, and a platform result completes
//! a session only while that session's generation is still the live one — a
//! result reported after the session finished or was abandoned is discarded
//! rather than resolving whatever session happens to be live now. The
//! generation is bookkeeping on this side of the platform boundary, not
//! evidence about the redirect: on the Android Custom Tabs fallback the host
//! stamps whichever callback-scheme `Intent` it observes with the generation
//! pending at that moment (the `android` module's *The generation round
//! trip*), so a stale tab's late redirect can still complete the live
//! session. An Auth Tab session is answered by the activity result of the
//! tab it launched instead, which no other `Intent` can produce. PKCE and
//! `state` (*Security* below) are what bind a callback to a request.
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
//! dropped future only frees this crate's bookkeeping: on Android the Auth
//! Tab or Custom Tab stays on screen until the user closes it (a redirect it
//! delivers afterwards finds no pending session and is dropped); on iOS/macOS the
//! presented `ASWebAuthenticationSession` sheet stays up until the user
//! dismisses it or the next [`AuthSession::start`] cancels it in favour of
//! the new session. To end a live session programmatically, start it with
//! [`AuthSession::start_with_cancel`] instead and call
//! [`AuthSessionCancel::cancel`]: the future resolves
//! [`AuthSessionOutcome::Cancelled`], the slot is freed, and the backend is
//! asked to tear its UI down (`-cancel` on Apple; on Android, where no API
//! closes an Auth Tab or a Custom Tab, the tab stays open until the user
//! leaves it — see
//! [`AuthSessionCancel`]).
//!
//! # Security
//!
//! **Callers MUST use PKCE (RFC 7636, `S256`) and MUST verify the `state`
//! parameter they generated before exchanging the authorization code.** No
//! platform can prove a callback came from the session it launched: on the
//! Android Custom Tabs fallback the redirect arrives as an ordinary `Intent`
//! that any app registering the same custom scheme can deliver, and while iOS/macOS
//! intercept the redirect in-process inside the session's own browser tab,
//! the redirect itself is still an unauthenticated HTTP response. An
//! [`AuthSessionOutcome::Callback`] is therefore evidence that *a* redirect
//! to the requested scheme arrived — never that the identity provider this
//! session presented is the party that sent it. PKCE binds the code to the
//! code verifier this app generated, and the `state` check binds the
//! callback to the request this app made; without both, a callback that
//! reaches this future is enough to complete someone else's flow.
//!
//! **The `https` form.** An [`AuthCallback::Https`] session catches its
//! redirect in-session only when the callback host is a domain associated
//! with the app. When it is not, the session does not catch the redirect —
//! it stays in the browser, or opens as an ordinary link — and the future
//! resolves only once the session is dismissed (by the user, or through
//! [`AuthSessionCancel::cancel`]). A URL that does come back is still checked
//! for scheme, host and path before it is handed over; that is a consistency
//! check on the platform's delivery, not proof of where the redirect came
//! from, so PKCE and `state` bind here exactly as they do for a custom scheme.
//! On Android the `https` form runs only in an Auth Tab, and the browser
//! verifies the app's claim on the domain through Digital Asset Links
//! (`assetlinks.json` served by the callback host); a failed or timed-out
//! verification resolves as [`AuthSessionError::Platform`] naming which.
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
//! 3. A cancel, dropping the future, or [`LoopbackOptions::timeout`] all
//!    take effect within about one poll interval (25 ms), even while a
//!    connection is being served — the one exception is a response write
//!    already in progress, bounded by the 2 s connection I/O timeout (see
//!    [`LoopbackSession::start`] and [`LoopbackCancel`] for the residual
//!    local-stall bound). The listener accepts exactly one callback, then stops.
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
//! | Android | Auth Tab where the browser offers it, else Chrome Custom Tabs (`android` module) | [`AuthSessionError::NoHandler`] |
//! | iOS | `ASWebAuthenticationSession` (`apple` module) | [`AuthSessionError::NoHandler`] |
//! | macOS | `ASWebAuthenticationSession` (`apple` module) | system browser + `127.0.0.1` listener |
//! | Linux, Windows | [`AuthSessionError::NoHandler`] (`unsupported` module) | system browser + `127.0.0.1` listener |
//! | anything else | [`AuthSessionError::NoHandler`] | [`AuthSessionError::NoHandler`] |
//!
//! [`AuthSession::is_supported`] answers the first column only — `true` on
//! the Android, iOS and macOS rows, `false` on Linux and Windows. The second
//! column has its own predicate, [`LoopbackSession::is_supported`] (`true`
//! on the macOS and Linux/Windows rows); a desktop caller asks that one.
//!
//! The first column is about [`AuthCallback::CustomScheme`] requests. An
//! [`AuthCallback::Https`] request (a claimed `https` redirect, the form RFC
//! 8252 §7.2 prefers where a platform supports it) is validated like any
//! other, then started only where a backend can catch it: on Apple systems
//! that have the `ASWebAuthenticationSessionCallback` API (iOS 17.4 or later,
//! macOS 14.4 or later), and on Android when the browser the plugin picks
//! offers Auth Tab (Chrome 137 or later) and the app's domain serves Digital
//! Asset Links. On Android without Auth Tab, on targets without a backend, and
//! on older Apple systems it is refused with
//! [`AuthSessionError::HttpsCallbackUnsupported`], before the busy slot is
//! claimed and before any UI appears.
//!
//! # The Android double-delivery note
//!
//! This note holds for the Custom Tabs fallback only. An Auth Tab session
//! gets its redirect as an activity result and not as a deep link, so it is
//! delivered once and the app needs no intent filter for the callback.
//!
//! On the Custom Tabs fallback, the callback URL redirected back into the app
//! reaches the platform **twice**: once as the redirect this crate's Android
//! backend resolves [`AuthSession::start`]'s future with, and once more as an
//! ordinary deep link the app's own `frust::deep_links()` stream also
//! observes (the OS has no way to know only this crate wants that intent).
//! Treat the future as the single authoritative source for an auth-session
//! callback and ignore the request's callback scheme in the app's own
//! deep-link handling — acting on both delivers the same authorization code
//! twice.
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

/// Where the identity provider redirects back to when an [`AuthSession`]
/// completes — the `redirect_uri` the authorization URL names, in the form
/// the platform needs to catch it.
///
/// Neither form is secret, so the derived `Debug` prints the scheme, or the
/// host and path, in full.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthCallback {
    /// A custom URL scheme, without `:` or `//` (e.g. `myapp`,
    /// `com.example.app`): any callback URL whose scheme is this one
    /// completes the session. 1-64 bytes, a lowercase ASCII letter first,
    /// then lowercase ASCII letters, digits, `+`, `-` or `.`, and not one of
    /// the well-known schemes (`http`, `https`, `file`, `javascript`,
    /// `data`, `blob`, `intent`, `content`, `about`).
    CustomScheme(String),
    /// A claimed `https` redirect (RFC 8252 §7.2): a callback URL completes
    /// the session only if its scheme is `https`, its host is [`host`]
    /// (compared ASCII-case-insensitively) and its path is exactly
    /// [`path`]; its query and fragment are handed back untouched.
    ///
    /// `host` is a bare DNS name — ASCII letters, digits, `-` and `.` only,
    /// so no scheme, port or userinfo; at most 253 bytes in labels of 1-63
    /// bytes that neither start nor end with `-`. `path` starts with `/`,
    /// then holds only RFC 3986 path characters — ASCII letters and digits,
    /// `-` `.` `_` `~`, `!` `$` `&` `'` `(` `)` `*` `+` `,` `;` `=`, `:`
    /// `@` `/`, and `%` followed by exactly two hex digits — with no `.` or
    /// `..` segment. The path is compared byte for byte with the delivered
    /// URL's, so write a percent-escape exactly as the provider sends it.
    ///
    /// Started on Apple systems with the callback API (iOS 17.4 or later,
    /// macOS 14.4 or later) and on Android when the browser offers Auth Tab
    /// (Chrome 137 or later), where the app's domain must serve Digital Asset
    /// Links; elsewhere [`AuthSession::start`] answers
    /// [`AuthSessionError::HttpsCallbackUnsupported`] for one.
    ///
    /// [`host`]: AuthCallback::Https::host
    /// [`path`]: AuthCallback::Https::path
    Https {
        /// The redirect's host, e.g. `login.example.com`.
        host: String,
        /// The redirect's path, e.g. `/oauth/callback`.
        path: String,
    },
}

/// A request to start an [`AuthSession`] — the identity provider's
/// authorization URL plus the [`AuthCallback`] it will redirect back to.
/// Build one with [`Self::custom_scheme`] or [`Self::https`], then
/// [`Self::ephemeral`](method@Self::ephemeral) if wanted.
///
/// `Debug`-formats without ever printing [`Self::url`] (this crate doc's
/// *The URL never appears in a `Display`/`Debug` string* section): a hand
/// implementation prints only the URL's byte length and the (non-secret)
/// callback.
#[non_exhaustive]
#[derive(Clone)]
pub struct AuthSessionRequest {
    /// The identity provider's authorization endpoint — must be an absolute
    /// `https` URL (see [`AuthSession::start`]'s validation).
    pub url: String,
    /// Where the identity provider redirects back to on completion —
    /// validated against [`AuthCallback`]'s rules by [`AuthSession::start`].
    pub callback: AuthCallback,
    /// Request a private/ephemeral browsing session with no persisted
    /// cookies or history
    /// (`ASWebAuthenticationSession.prefersEphemeralWebBrowserSession` on
    /// Apple; `CustomTabsIntent.Builder#setEphemeralBrowsingEnabled` on
    /// Android, where it is **advisory** — a browser that does not
    /// implement it silently runs an ordinary session instead).
    pub ephemeral: bool,
}

impl AuthSessionRequest {
    /// A non-ephemeral request for `url` that completes on a redirect to the
    /// custom scheme `scheme` ([`AuthCallback::CustomScheme`]).
    pub fn custom_scheme(url: impl Into<String>, scheme: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            callback: AuthCallback::CustomScheme(scheme.into()),
            ephemeral: false,
        }
    }

    /// A non-ephemeral request for `url` that completes on a redirect to
    /// `https://<host><path>` ([`AuthCallback::Https`]). `path` must start
    /// with `/` and hold only RFC 3986 path characters, a `%` always followed
    /// by two hex digits, with no `.` or `..` segment (the full rule is on
    /// [`AuthCallback::Https`]); it is compared byte for byte with the
    /// delivered URL's path, so write a percent-escape exactly as the
    /// provider sends it. [`AuthSession::start`] checks both rules.
    pub fn https(url: impl Into<String>, host: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            callback: AuthCallback::Https {
                host: host.into(),
                path: path.into(),
            },
            ephemeral: false,
        }
    }

    /// Set [`Self::ephemeral`](field@Self::ephemeral) — see that field.
    #[must_use]
    pub fn ephemeral(mut self, ephemeral: bool) -> Self {
        self.ephemeral = ephemeral;
        self
    }
}

impl fmt::Debug for AuthSessionRequest {
    /// Never prints [`Self::url`] (an authorization URL can carry
    /// provider-specific session state) — only its byte length and the
    /// callback, which is not secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthSessionRequest")
            .field("url_len", &self.url.len())
            .field("callback", &self.callback)
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
///
/// # Leaving the app while a session is live
///
/// What the future resolves to when the user switches away from the app
/// while a custom-scheme session is live differs per platform:
///
/// - **Android, Auth Tab**: the session is answered only by the activity
///   result of the tab it launched. An unrelated Activity resume never
///   resolves it [`Self::Cancelled`]; it resolves on the redirect, the user
///   closing the tab, or [`AuthSessionCancel::cancel`].
/// - **Android, Custom Tabs fallback**: nothing resolves while the app is in
///   the background, and
///   returning to a task whose top is still the Custom Tab resumes the tab,
///   not the app, so that resolves nothing either. The first time one of
///   the app's own Activities resumes *without* the callback intent — the
///   user backing out of the tab, or returning to an app Activity by any
///   route other than the redirect — the session resolves
///   [`Self::Cancelled`], indistinguishable from a dismissed tab
///   (`auth-session-android-resume-means-cancelled-v1` in
///   `docs/LIMITATIONS.md`). A tab still open at that point stays open.
/// - **iOS / macOS**: this crate watches no app-lifecycle event; the future
///   resolves only from the `ASWebAuthenticationSession` completion handler.
///   Backgrounding the app reports no completion by itself, so the future
///   stays pending and the sheet is still presented when the app returns;
///   it resolves on the redirect, the user's own dismissal
///   ([`Self::Cancelled`]), [`AuthSessionCancel::cancel`], or a platform
///   error.
/// - **[`LoopbackSession`]** (desktop): the listener keeps waiting whether
///   or not the app is focused, bounded by [`LoopbackOptions::timeout`].
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq)]
pub enum AuthSessionOutcome {
    /// The identity provider redirected back to the request's
    /// [`AuthCallback`] — the full callback URL, including its query string
    /// (the authorization code and any `state`) and any fragment.
    Callback(String),
    /// The session ended before the identity provider ever redirected: the
    /// user dismissed the in-app browser tab, the app called
    /// [`AuthSessionCancel::cancel`] (or [`LoopbackCancel::cancel`]), or — on
    /// the Android Custom Tabs fallback — an Activity resumed without the callback (see *Leaving the
    /// app while a session is live* above).
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
    /// narrowing of it), or [`AuthSessionRequest::callback`] breaks its
    /// form's rules: a custom scheme outside this crate's scheme rule table
    /// (1-64 bytes, lowercase-alpha first byte, lowercase-alnum/`+`/`-`/`.`
    /// thereafter, not one of the denied well-known schemes), or an `https`
    /// host that is not a bare DNS name or a path outside the path rule (see
    /// [`AuthCallback::Https`]).
    #[error("auth session: request URL is not an absolute https URL or the callback is invalid")]
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
    /// refuses to hand back: one delivered with no URL at all, or one that
    /// does not match the [`AuthSessionRequest::callback`] the session asked
    /// for (a different scheme; for an `https` callback, also a different
    /// host or path).
    #[error("auth session error: {0}")]
    Platform(String),

    /// The request names an [`AuthCallback::Https`] redirect, and this
    /// target's backend cannot start a session that catches one. Answered
    /// after validation, before the busy slot is claimed and before any UI
    /// appears — so it never makes another request `Busy`. Answered on
    /// Android when the browser offers no Auth Tab, on targets without a
    /// backend, and on Apple systems older than iOS 17.4 / macOS 14.4; a
    /// custom-scheme request is unaffected.
    #[error("auth session: https callbacks are not supported on this platform")]
    HttpsCallbackUnsupported,
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

/// The [`AuthSessionError::Platform`] message for a callback URL that does
/// not match the session's [`AuthCallback::Https`] redirect: not `https`, or
/// a different host or path. Names neither the requested host and path nor
/// the URL, for the same reason as [`CALLBACK_SCHEME_MISMATCH`].
const CALLBACK_HTTPS_MISMATCH: &str =
    "callback URL does not match the requested https host and path";

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
    /// What a delivered callback URL must match, kept so [`resolve`] can
    /// re-check it rather than trusting the platform to have filtered.
    callback: LiveCallback,
    /// The channel half that completes the caller's future.
    sender: oneshot::Sender,
}

/// What [`checked_callback`] checks a live session's callback URL against.
#[derive(Debug, PartialEq, Eq)]
enum LiveCallback {
    /// An [`AuthSession`]: the [`AuthSessionRequest::callback`] it asked for.
    Request(AuthCallback),
    /// A [`LoopbackSession`] listener: the URL must be `http`. The listener's
    /// own request checks have already pinned its host, port and path.
    Loopback,
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

/// Cancel generation `generation`'s session — [`AuthSessionCancel::cancel`]'s
/// whole body. If that session is still the live one, it is completed with
/// [`AuthSessionOutcome::Cancelled`] (exactly once: [`take_live`] empties the
/// slot under the lock, so a racing platform result or a second cancel finds
/// nothing), the Busy slot is thereby free, and only then is the backend
/// asked to tear down its UI. A finished session, an abandoned one, or a
/// newer generation is left untouched and the backend is never called. A
/// platform result that arrives for `generation` afterwards finds no live
/// session of that generation and is discarded by [`resolve`].
fn cancel_generation(generation: u64) {
    let Some(live) = take_live(generation) else {
        return;
    };
    // The lock is already released (`take_live`): sending wakes the caller,
    // whose continuation may start the next session — which must not
    // deadlock on `ACTIVE`, and which the backend hook below cannot touch,
    // since that hook is itself guarded by `generation`.
    live.sender.send(Ok(AuthSessionOutcome::Cancelled));
    backend::cancel(generation);
}

/// The Apple backend's dismissal-grace bookkeeping: when it last sent a
/// session `-cancel`, and so how much longer a new session's `-start` must
/// wait for that sheet to finish animating out (`apple`'s module doc,
/// *Waiting out a dismissal*). Pure arithmetic over caller-supplied
/// instants, with no Apple API in it, so it lives here — where the host test
/// binary can reach it — rather than in the `target_vendor = "apple"`-only
/// `apple` module, which keeps one in its main-thread state.
#[cfg(any(target_vendor = "apple", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DismissalGrace {
    /// When the most recent `-cancel` was sent; `None` before the first.
    cancelled_at: Option<std::time::Instant>,
}

#[cfg(any(target_vendor = "apple", test))]
impl DismissalGrace {
    /// No `-cancel` sent yet: nothing to wait for.
    pub(crate) const fn new() -> Self {
        Self { cancelled_at: None }
    }

    /// A session was sent `-cancel` at `now`; the grace window restarts
    /// from it, whatever was recorded before.
    pub(crate) fn record_cancel(&mut self, now: std::time::Instant) {
        self.cancelled_at = Some(now);
    }

    /// How much longer a `-start` must wait at `now` for a dismissal that
    /// takes `grace` — `None` when no `-cancel` was ever recorded or the
    /// last one is at least `grace` old, so a start with no recent cancel is
    /// never delayed.
    pub(crate) fn remaining(
        &self,
        now: std::time::Instant,
        grace: std::time::Duration,
    ) -> Option<std::time::Duration> {
        let elapsed = now.saturating_duration_since(self.cancelled_at?);
        grace.checked_sub(elapsed).filter(|left| !left.is_zero())
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
/// against the live session's own [`AuthCallback`] before it completes
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
    let outcome = checked_callback(&live.callback, outcome);
    live.sender.send(outcome);
}

/// Re-check a delivered callback URL against `callback`, the redirect the
/// live session actually asked for. Anything that is not an
/// `Ok(`[`AuthSessionOutcome::Callback`]`)` passes through untouched, and a
/// matching URL passes through byte-for-byte — query and fragment included.
///
/// The rejections, all [`AuthSessionError::Platform`] and none naming the
/// URL (this crate doc's URL rule):
///
/// - an empty URL — a host that reported a callback but carried nothing in
///   it, which a caller would otherwise parse as a real answer;
/// - for [`AuthCallback::CustomScheme`], a URL whose scheme (the bytes
///   before its first `:`, compared ASCII-case-insensitively —
///   [`validate_callback_scheme`] already restricts the requested scheme to
///   lowercase) is not the requested one, or which has no scheme at all
///   ([`CALLBACK_SCHEME_MISMATCH`]); for [`LiveCallback::Loopback`], the
///   same check against `http`;
/// - for [`AuthCallback::Https`], a URL that fails
///   [`https_callback_matches`] ([`CALLBACK_HTTPS_MISMATCH`]).
///
/// This is a consistency check on the platform's own delivery, **not** an
/// authenticity check: a matching scheme, host or path proves nothing about
/// who sent the redirect (this crate doc's *Security* section).
fn checked_callback(
    callback: &LiveCallback,
    outcome: Result<AuthSessionOutcome, AuthSessionError>,
) -> Result<AuthSessionOutcome, AuthSessionError> {
    let Ok(AuthSessionOutcome::Callback(url)) = &outcome else {
        return outcome;
    };
    if url.is_empty() {
        return Err(AuthSessionError::Platform(CALLBACK_WITHOUT_URL.to_string()));
    }
    let (matches, mismatch) = match callback {
        LiveCallback::Request(AuthCallback::CustomScheme(scheme)) => {
            (scheme_matches(url, scheme), CALLBACK_SCHEME_MISMATCH)
        }
        LiveCallback::Loopback => (scheme_matches(url, "http"), CALLBACK_SCHEME_MISMATCH),
        LiveCallback::Request(AuthCallback::Https { host, path }) => (
            https_callback_matches(url, host, path),
            CALLBACK_HTTPS_MISMATCH,
        ),
    };
    if matches {
        outcome
    } else {
        Err(AuthSessionError::Platform(mismatch.to_string()))
    }
}

/// Whether `url`'s scheme — the bytes before its first `:` — is `scheme`,
/// compared ASCII-case-insensitively. A URL with no `:` has no scheme and
/// never matches.
fn scheme_matches(url: &str, scheme: &str) -> bool {
    url.find(':')
        .is_some_and(|colon| url[..colon].eq_ignore_ascii_case(scheme))
}

/// Whether `url` is a redirect to `https://<host><path>`: scheme `https`
/// (ASCII-case-insensitive), then `//`, then an authority equal to `host`
/// (ASCII-case-insensitive — so a port or userinfo never matches, since
/// [`validate_https_host`] admits neither), then a path — everything up to
/// the first `?` or `#` — equal to `path` byte for byte. Whatever follows
/// the path (query, fragment) is not looked at.
fn https_callback_matches(url: &str, host: &str, path: &str) -> bool {
    let Some(colon) = url.find(':') else {
        return false;
    };
    if !url[..colon].eq_ignore_ascii_case("https") {
        return false;
    }
    let Some(rest) = url[colon + 1..].strip_prefix("//") else {
        return false;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(authority_end);
    if !authority.eq_ignore_ascii_case(host) {
        return false;
    }
    let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
    &rest[..path_end] == path
}

/// Validate `req` against this crate's URL and callback rule tables.
/// `Ok(())` means `req` is safe to hand to a backend.
fn validate(req: &AuthSessionRequest) -> Result<(), AuthSessionError> {
    validate_https_url(&req.url)?;
    match &req.callback {
        AuthCallback::CustomScheme(scheme) => validate_callback_scheme(scheme),
        AuthCallback::Https { host, path } => {
            validate_https_host(host)?;
            validate_https_path(path)
        }
    }
}

/// Validate an [`AuthCallback::Https`] host: a bare DNS name of at most 253
/// bytes, ASCII letters (either case — matching compares
/// case-insensitively), digits, `-` and `.` only, so no scheme, port,
/// userinfo or path can ride along; every `.`-separated label 1-63 bytes,
/// neither starting nor ending with `-` (so no leading, trailing or doubled
/// `.` either).
fn validate_https_host(host: &str) -> Result<(), AuthSessionError> {
    if host.is_empty() || host.len() > 253 {
        return Err(AuthSessionError::InvalidUrl);
    }
    let label_ok = |label: &str| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes
                .iter()
                .all(|&byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && bytes.first() != Some(&b'-')
            && bytes.last() != Some(&b'-')
    };
    if host.split('.').all(label_ok) {
        Ok(())
    } else {
        Err(AuthSessionError::InvalidUrl)
    }
}

/// Validate an [`AuthCallback::Https`] path: a `/`, then only RFC 3986 path
/// bytes ([`is_https_path_byte`]) and `%` + two hex digits, with no `.` or
/// `..` segment. The delivered URL carries its path percent-encoded and
/// [`https_callback_matches`] compares byte for byte, so a path holding
/// anything else (a space, a non-ASCII byte, a stray `%`, a `?` or `#`) could
/// never match and would lose the genuine redirect as a mismatch.
fn validate_https_path(path: &str) -> Result<(), AuthSessionError> {
    let Some(rest) = path.strip_prefix('/') else {
        return Err(AuthSessionError::InvalidUrl);
    };
    let bytes = rest.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let escape = bytes.get(index + 1..index + 3);
            if !escape.is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)) {
                return Err(AuthSessionError::InvalidUrl);
            }
            index += 3;
        } else if is_https_path_byte(bytes[index]) {
            index += 1;
        } else {
            return Err(AuthSessionError::InvalidUrl);
        }
    }
    if rest
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(AuthSessionError::InvalidUrl);
    }
    Ok(())
}

/// An RFC 3986 path byte other than a percent-escape: `unreserved`
/// (ASCII letters and digits, `-` `.` `_` `~`), `sub-delims`
/// (`!` `$` `&` `'` `(` `)` `*` `+` `,` `;` `=`), `:`, `@` and `/`.
fn is_https_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
                | b'/'
        )
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
    /// Validate `req`, refuse an `https` callback the backend cannot
    /// start, claim the slot and hand the session to the backend. Also
    /// returns the generation the session was accepted under — `None` when
    /// it never reached a backend-presented state (a validation failure,
    /// [`AuthSessionError::HttpsCallbackUnsupported`],
    /// [`AuthSessionError::Busy`], or the backend's own synchronous
    /// refusal), which is what makes [`AuthSession::start_with_cancel`]'s
    /// handle inert on exactly those paths.
    fn begin(req: AuthSessionRequest) -> (Self, Option<u64>) {
        Self::begin_with(req, backend::https_callback_supported, backend::start)
    }

    /// [`Self::begin`] with the backend's two entry points injected — the
    /// seam that keeps unit tests from ever handing an accepted request to a
    /// real platform backend (`loopback`'s `start_with_opener` precedent).
    fn begin_with(
        req: AuthSessionRequest,
        https_callback_supported: impl FnOnce() -> bool,
        start: impl FnOnce(AuthSessionRequest, SessionToken) -> Result<(), AuthSessionError>,
    ) -> (Self, Option<u64>) {
        if let Err(err) = validate(&req) {
            return (StartFuture::Ready(Some(Err(err))), None);
        }

        // The one place that asks a backend whether it can catch an `https`
        // redirect: before the slot is claimed, so the refusal never makes
        // another request `Busy`, and before the backend presents anything.
        if matches!(req.callback, AuthCallback::Https { .. }) && !https_callback_supported() {
            return (
                StartFuture::Ready(Some(Err(AuthSessionError::HttpsCallbackUnsupported))),
                None,
            );
        }

        let mut active = lock_active();
        if active.is_some() {
            return (StartFuture::Ready(Some(Err(AuthSessionError::Busy))), None);
        }

        let generation = next_generation();
        let (sender, receiver) = oneshot::channel(generation);
        *active = Some(Live {
            generation,
            callback: LiveCallback::Request(req.callback.clone()),
            sender,
        });
        // Release the lock before calling into the backend: a backend whose
        // early-failure path resolves synchronously (via `resolve`) must not
        // deadlock on a lock this function itself is still holding.
        drop(active);

        match start(req, SessionToken { generation }) {
            Ok(()) => (StartFuture::Pending(receiver), Some(generation)),
            Err(err) => {
                // The backend rejected the request before ever presenting a
                // session UI (`unsupported`'s blanket refusal, or a real
                // backend's `PlatformNotInitialized` check) — release the
                // slot and resolve the receiver with that error, rather than
                // leaving it live for nothing to ever complete.
                resolve(generation, Err(err));
                (StartFuture::Pending(receiver), None)
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
    /// an absolute `https` URL or [`AuthSessionRequest::callback`] breaks
    /// [`AuthCallback`]'s rules — checked before any platform API is
    /// touched, and resolved on the future's first poll rather than
    /// deferred. [`AuthSessionError::HttpsCallbackUnsupported`] for a valid
    /// [`AuthCallback::Https`] request on a backend that cannot catch one
    /// (Android without Auth Tab, targets without a backend, and Apple
    /// systems older than iOS 17.4 / macOS 14.4), also resolved on first poll and without
    /// claiming the busy slot. [`AuthSessionError::Busy`] if another
    /// [`AuthSession`] is already live, also resolved on first poll.
    /// Otherwise, per platform:
    /// [`AuthSessionError::PlatformNotInitialized`]/
    /// [`AuthSessionError::NoHandler`]/[`AuthSessionError::Platform`] for a
    /// backend-specific pre-dispatch failure (resolved on first poll), or —
    /// once a session is genuinely presented — [`AuthSessionOutcome`] or a
    /// later [`AuthSessionError::Platform`] once the platform actually
    /// responds.
    ///
    /// A session started here cannot be cancelled programmatically; use
    /// [`Self::start_with_cancel`] when the app may need to end it itself.
    /// What the future resolves to when the user leaves the app mid-session
    /// is listed on [`AuthSessionOutcome`] (*Leaving the app while a session
    /// is live*).
    pub fn start(
        req: AuthSessionRequest,
    ) -> impl Future<Output = Result<AuthSessionOutcome, AuthSessionError>> + Send + 'static {
        StartFuture::begin(req).0
    }

    /// [`Self::start`], plus an [`AuthSessionCancel`] handle that ends this
    /// one session from any thread.
    ///
    /// The future behaves exactly like [`Self::start`]'s, with the same
    /// validation, Busy guard, errors and security contract (this crate
    /// doc's *Security* section) — the handle adds a way out and changes
    /// nothing else. A request refused at start ([`AuthSessionError::
    /// InvalidUrl`], [`AuthSessionError::HttpsCallbackUnsupported`],
    /// [`AuthSessionError::Busy`], or a backend's synchronous
    /// pre-dispatch failure such as [`AuthSessionError::NoHandler`]) comes
    /// back with an **inert** handle: cancelling it does nothing, and in
    /// particular never touches the session whose presence made this
    /// request `Busy`.
    ///
    /// While the browser is in front the app is in the background, where its
    /// UI-thread timers (`frust::time`) do not run: a cancel scheduled on one
    /// fires only when the user comes back. To end a session on a deadline,
    /// call the handle from another thread; it is `Send + Sync` for that.
    pub fn start_with_cancel(
        req: AuthSessionRequest,
    ) -> (
        impl Future<Output = Result<AuthSessionOutcome, AuthSessionError>> + Send + 'static,
        AuthSessionCancel,
    ) {
        let (future, generation) = StartFuture::begin(req);
        (future, AuthSessionCancel { generation })
    }

    /// Whether [`Self::start`] (a custom-scheme callback) has a backend on
    /// this build target: Android (Auth Tab or Chrome Custom Tabs) and Apple
    /// (`target_vendor = "apple"`, so iOS and macOS both, on
    /// `ASWebAuthenticationSession`). `false` on Linux and Windows, where
    /// [`Self::start`] answers [`AuthSessionError::NoHandler`] — the desktop
    /// path is [`LoopbackSession`], which has its own
    /// [`LoopbackSession::is_supported`] (the crate doc's *Platform notes*
    /// table). The two predicates answer two different questions on purpose;
    /// this one never says `true` for a target where `start` cannot work.
    /// It also says nothing about [`AuthCallback::Https`]: whether that form
    /// can start is decided per request at `start` time (Apple with the
    /// callback API; Android when the browser offers Auth Tab).
    ///
    /// A compile-time answer about the backends this target builds, not a
    /// runtime probe: `true` on Android still leaves
    /// [`AuthSessionError::NoHandler`] reachable at `start` time on a device
    /// with no Custom Tabs-capable browser installed.
    pub fn is_supported() -> bool {
        cfg!(any(target_os = "android", target_vendor = "apple"))
    }
}

/// Cancels the one [`AuthSession`] [`AuthSession::start_with_cancel`]
/// returned it for, from any thread — the custom-scheme counterpart of
/// [`LoopbackCancel`].
///
/// While that session is still the live one, [`Self::cancel`] resolves its
/// future with [`AuthSessionOutcome::Cancelled`] (exactly once), frees the
/// one-live-session slot (this crate doc's *Exactly one live session*) so
/// the next [`AuthSession::start`] is accepted at once — never refused
/// [`AuthSessionError::Busy`] — and then asks the platform to tear the
/// browser UI down:
///
/// - **iOS / macOS**: the retained `ASWebAuthenticationSession` is sent
///   `-cancel` on the main queue, which dismisses its sheet. A completion
///   the framework still reports afterwards is discarded as a stale
///   generation. A session started right after the cancel is accepted at
///   once too, but its own sheet appears only after a short dismissal delay
///   (half a second at most) that lets the cancelled one finish animating
///   out — the platform refuses to present over a sheet still on its way
///   down.
/// - **Android**: there is no API to close an Auth Tab or a Custom Tab. The
///   host forgets the pending session, and a launch for it that had not run
///   yet opens no tab; a tab already open **stays open** in front of the app
///   until the user leaves it, and a redirect it delivers later finds no
///   pending session and is dropped. On the Custom Tabs fallback that
///   redirect still reaches the app's own deep-link stream (this crate
///   doc's *The Android double-delivery note*); an Auth Tab's does not.
///
/// Every step is keyed to the session's generation: once that session has
/// finished (by outcome, error, a previous cancel, or the caller dropping
/// its future), `cancel` does nothing — it never ends, frees or tears down a
/// newer session. A platform result that arrives later for the cancelled
/// session is discarded. A handle returned for a request refused at start is
/// inert ([`AuthSession::start_with_cancel`]).
#[derive(Clone)]
pub struct AuthSessionCancel {
    /// The generation of the session this handle cancels; `None` for an
    /// inert handle.
    generation: Option<u64>,
}

impl AuthSessionCancel {
    /// Request cancellation — see the type's doc.
    pub fn cancel(&self) {
        if let Some(generation) = self.generation {
            cancel_generation(generation);
        }
    }
}

impl fmt::Debug for AuthSessionCancel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthSessionCancel").finish_non_exhaustive()
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
    use std::time::{Duration, Instant};

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
        AuthSessionRequest::custom_scheme("https://issuer.example/authorize", "frustplay")
    }

    /// A valid [`AuthCallback::Https`] request.
    fn https_request() -> AuthSessionRequest {
        AuthSessionRequest::https(
            "https://login.example.com/authorize",
            "app.example.com",
            "/oauth/callback",
        )
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

    /// A reverse-DNS callback scheme — the shape Apple and Android apps
    /// conventionally register — is accepted.
    #[test]
    fn reverse_dns_callback_scheme_is_accepted() {
        assert_eq!(validate_callback_scheme("io.example.console"), Ok(()));
        assert_eq!(validate_callback_scheme("com.example.app"), Ok(()));
    }

    #[test]
    fn is_supported_answers_the_custom_scheme_column_only() {
        assert_eq!(
            AuthSession::is_supported(),
            cfg!(any(target_os = "android", target_vendor = "apple"))
        );
        if cfg!(any(target_os = "linux", target_os = "windows")) {
            // Desktop without an in-app browser tab: `start` cannot work, the
            // loopback path can — two predicates, two answers.
            assert!(!AuthSession::is_supported());
            assert!(LoopbackSession::is_supported());
        }
        if cfg!(target_os = "macos") {
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
    fn live_session(scheme: &str) -> (u64, StartFuture) {
        live_session_for(AuthCallback::CustomScheme(scheme.to_string()))
    }

    /// [`live_session`] for any [`AuthCallback`].
    fn live_session_for(callback: AuthCallback) -> (u64, StartFuture) {
        let generation = next_generation();
        let (sender, receiver) = oneshot::channel(generation);
        *lock_active() = Some(Live {
            generation,
            callback: LiveCallback::Request(callback),
            sender,
        });
        (generation, StartFuture::Pending(receiver))
    }

    /// [`AuthSession::start_with_cancel`] against a stand-in backend that
    /// accepts every request (an `https` one included) and presents nothing.
    /// No test hands a request a real backend would accept to that backend:
    /// on Apple it would queue a real session on the main queue, so every
    /// accepted path below — custom scheme and `https` — goes through this
    /// instead, and the Apple presentation path is covered at the device
    /// gate only. Shared with the `loopback` tests.
    pub(crate) fn start_with_accepting_backend(
        req: AuthSessionRequest,
    ) -> (StartFuture, AuthSessionCancel) {
        let (future, generation) = StartFuture::begin_with(req, || true, |_req, _token| Ok(()));
        (future, AuthSessionCancel { generation })
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

    /// A backend that accepts (Apple hands the present sequence to the main
    /// queue and returns at once) leaves the future genuinely pending, with
    /// the slot claimed for the request's callback until a completion, a
    /// cancel or the caller's drop frees it.
    #[test]
    fn an_accepting_backend_leaves_the_session_pending_with_the_slot_claimed() {
        with_clean_active(|| {
            let (mut fut, _handle) = start_with_accepting_backend(valid_request());
            assert_eq!(poll_once(&mut fut), Poll::Pending);

            let active = lock_active();
            let live = active.as_ref().expect("the slot must stay claimed");
            assert_ne!(live.generation, 0, "a generation is never zero");
            assert_eq!(
                live.callback,
                LiveCallback::Request(AuthCallback::CustomScheme("frustplay".to_string()))
            );
        });
    }

    /// A cancel handle for `generation`, as
    /// [`AuthSession::start_with_cancel`] would have returned for a session
    /// [`live_session`] claimed.
    fn cancel_handle(generation: u64) -> AuthSessionCancel {
        AuthSessionCancel {
            generation: Some(generation),
        }
    }

    #[test]
    fn the_cancel_handle_is_clone_send_sync_and_debug_prints_nothing() {
        fn assert_traits<T: Clone + Send + Sync + fmt::Debug + 'static>() {}
        assert_traits::<AuthSessionCancel>();
        assert_eq!(
            format!("{:?}", cancel_handle(7)),
            "AuthSessionCancel { .. }"
        );
    }

    #[test]
    fn cancel_resolves_the_live_session_cancelled_exactly_once() {
        with_clean_active(|| {
            let (generation, mut fut) = live_session("frustplay");
            let handle = cancel_handle(generation);
            assert_eq!(poll_once(&mut fut), Poll::Pending);

            handle.cancel();
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Ok(AuthSessionOutcome::Cancelled))
            );
            assert!(lock_active().is_none(), "cancel must release the slot");

            // A second cancel — from the same handle or a clone — completes
            // nothing more.
            handle.clone().cancel();
            handle.cancel();
            assert_eq!(poll_once(&mut fut), Poll::Pending);
            assert!(lock_active().is_none());
        });
    }

    #[test]
    fn cancel_after_completion_is_a_no_op() {
        with_clean_active(|| {
            let (generation, mut fut) = live_session("frustplay");
            resolve(
                generation,
                Ok(AuthSessionOutcome::Callback(
                    "frustplay://auth/callback?code=x".to_string(),
                )),
            );

            cancel_handle(generation).cancel();
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Ok(AuthSessionOutcome::Callback(
                    "frustplay://auth/callback?code=x".to_string()
                ))),
                "the outcome that completed first must stand"
            );
            assert_eq!(poll_once(&mut fut), Poll::Pending);
            assert!(lock_active().is_none());
        });
    }

    #[test]
    fn cancelling_generation_n_leaves_generation_n_plus_one_live() {
        with_clean_active(|| {
            let (first, first_fut) = live_session("frustplay");
            let stale = cancel_handle(first);
            // The caller abandons the first session; a newer one starts.
            drop(first_fut);
            let (second, mut second_fut) = live_session("frustplay");
            assert_ne!(first, second);

            stale.cancel();
            assert_eq!(poll_once(&mut second_fut), Poll::Pending);
            assert_eq!(
                lock_active().as_ref().map(|live| live.generation),
                Some(second),
                "a stale handle must leave the newer session live"
            );

            // The newer session's own handle still works.
            cancel_handle(second).cancel();
            assert_eq!(
                poll_once(&mut second_fut),
                Poll::Ready(Ok(AuthSessionOutcome::Cancelled))
            );
        });
    }

    #[test]
    fn busy_clears_after_cancel() {
        with_clean_active(|| {
            let (generation, _fut) = live_session("frustplay");
            let mut busy = AuthSession::start(valid_request());
            assert_eq!(
                poll_once(&mut busy),
                Poll::Ready(Err(AuthSessionError::Busy))
            );

            cancel_handle(generation).cancel();
            let (mut next, _handle) = start_with_accepting_backend(valid_request());
            assert_eq!(
                poll_once(&mut next),
                Poll::Pending,
                "the slot must be free for the next start after a cancel"
            );
        });
    }

    #[test]
    fn a_late_platform_result_for_a_cancelled_generation_is_ignored() {
        with_clean_active(|| {
            let (generation, mut fut) = live_session("frustplay");
            cancel_handle(generation).cancel();
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Ok(AuthSessionOutcome::Cancelled))
            );

            // With nothing live: the late redirect is discarded.
            resolve(
                generation,
                Ok(AuthSessionOutcome::Callback(
                    "frustplay://auth/callback?code=late".to_string(),
                )),
            );
            assert_eq!(poll_once(&mut fut), Poll::Pending);
            assert!(lock_active().is_none());

            // With a newer session live: the late result neither completes
            // nor releases it.
            let (newer, mut newer_fut) = live_session("frustplay");
            resolve(generation, Ok(AuthSessionOutcome::Cancelled));
            assert_eq!(poll_once(&mut newer_fut), Poll::Pending);
            assert_eq!(
                lock_active().as_ref().map(|live| live.generation),
                Some(newer)
            );
        });
    }

    #[test]
    fn an_inert_handle_does_nothing() {
        with_clean_active(|| {
            // Refused for an invalid URL: inert.
            let mut req = valid_request();
            req.url = "http://issuer.example/authorize".to_string();
            let (mut invalid, invalid_handle) = AuthSession::start_with_cancel(req);
            assert!(invalid_handle.generation.is_none());
            assert_eq!(
                poll_once(&mut invalid),
                Poll::Ready(Err(AuthSessionError::InvalidUrl))
            );

            // Refused as Busy: inert, and cancelling it must not end the
            // session whose presence made it Busy.
            let (generation, mut live_fut) = live_session("frustplay");
            let (mut busy, busy_handle) = AuthSession::start_with_cancel(valid_request());
            assert!(busy_handle.generation.is_none());
            assert_eq!(
                poll_once(&mut busy),
                Poll::Ready(Err(AuthSessionError::Busy))
            );

            invalid_handle.cancel();
            busy_handle.cancel();
            assert_eq!(poll_once(&mut live_fut), Poll::Pending);
            assert_eq!(
                lock_active().as_ref().map(|live| live.generation),
                Some(generation)
            );
        });
    }

    /// On a target whose backend refuses synchronously, the handle comes
    /// back inert too, and the future resolves exactly as `start`'s does.
    #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
    #[test]
    fn a_backend_refusal_returns_an_inert_handle() {
        with_clean_active(|| {
            let (mut fut, handle) = AuthSession::start_with_cancel(valid_request());
            assert!(handle.generation.is_none());
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Err(AuthSessionError::NoHandler))
            );
            handle.cancel();
            assert!(lock_active().is_none());
        });
    }

    /// A backend that accepts gets a live handle back; cancelling it
    /// resolves the future at once, before the backend has reported anything.
    #[test]
    fn an_accepted_start_returns_a_live_handle_that_cancels() {
        with_clean_active(|| {
            let (mut fut, handle) = start_with_accepting_backend(valid_request());
            assert!(handle.generation.is_some());
            assert_eq!(poll_once(&mut fut), Poll::Pending);

            handle.cancel();
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Ok(AuthSessionOutcome::Cancelled))
            );
            assert!(lock_active().is_none());
        });
    }

    /// The grace every [`DismissalGrace`] test waits out.
    const GRACE: Duration = Duration::from_millis(500);

    /// With no `-cancel` ever recorded, a start is never delayed.
    #[test]
    fn dismissal_grace_without_a_cancel_never_delays() {
        let grace = DismissalGrace::new();
        assert_eq!(grace.remaining(Instant::now(), GRACE), None);
    }

    /// A start right after a cancel waits the whole grace; one made part
    /// way through waits only what is left of it.
    #[test]
    fn dismissal_grace_counts_down_from_the_cancel() {
        let cancelled = Instant::now();
        let mut grace = DismissalGrace::new();
        grace.record_cancel(cancelled);

        assert_eq!(grace.remaining(cancelled, GRACE), Some(GRACE));
        assert_eq!(
            grace.remaining(cancelled + Duration::from_millis(200), GRACE),
            Some(Duration::from_millis(300))
        );
    }

    /// Once the grace has run out — exactly at its end or any time after —
    /// a start goes ahead at once.
    #[test]
    fn dismissal_grace_ends_after_the_delay() {
        let cancelled = Instant::now();
        let mut grace = DismissalGrace::new();
        grace.record_cancel(cancelled);

        assert_eq!(grace.remaining(cancelled + GRACE, GRACE), None);
        assert_eq!(
            grace.remaining(cancelled + Duration::from_secs(60), GRACE),
            None
        );
    }

    /// A later cancel restarts the window from its own instant.
    #[test]
    fn a_later_cancel_restarts_the_dismissal_grace() {
        let first = Instant::now();
        let second = first + Duration::from_millis(400);
        let mut grace = DismissalGrace::new();
        grace.record_cancel(first);
        grace.record_cancel(second);

        assert_eq!(
            grace.remaining(first + Duration::from_millis(600), GRACE),
            Some(Duration::from_millis(300))
        );
    }

    /// An instant earlier than the recorded cancel (never produced on one
    /// thread, but not ruled out by the type) counts as no time elapsed.
    #[test]
    fn dismissal_grace_clamps_an_instant_before_the_cancel() {
        let earlier = Instant::now();
        let cancelled = earlier + Duration::from_millis(100);
        let mut grace = DismissalGrace::new();
        grace.record_cancel(cancelled);

        assert_eq!(grace.remaining(earlier, GRACE), Some(GRACE));
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

            // The next start is accepted rather than rejected as `Busy`.
            let (mut next, _handle) = start_with_accepting_backend(valid_request());
            assert_eq!(poll_once(&mut next), Poll::Pending);
            drop(next);

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

    /// Every way to build a request lands in the same shape, and
    /// `ephemeral` only flips the one field.
    #[test]
    fn constructors_build_the_requested_callback() {
        let custom = AuthSessionRequest::custom_scheme("https://issuer.example/a", "frustplay");
        assert_eq!(custom.url, "https://issuer.example/a");
        assert_eq!(
            custom.callback,
            AuthCallback::CustomScheme("frustplay".to_string())
        );
        assert!(!custom.ephemeral);
        assert!(custom.ephemeral(true).ephemeral);

        let https = AuthSessionRequest::https(
            "https://login.example.com/a".to_string(),
            "app.example.com".to_string(),
            "/cb".to_string(),
        )
        .ephemeral(true);
        assert_eq!(
            https.callback,
            AuthCallback::Https {
                host: "app.example.com".to_string(),
                path: "/cb".to_string(),
            }
        );
        assert!(https.ephemeral);
        assert!(!https.ephemeral(false).ephemeral);
    }

    /// The request's `Debug` names the callback, never the URL.
    #[test]
    fn request_debug_prints_the_callback_and_only_the_url_length() {
        let custom =
            AuthSessionRequest::custom_scheme("https://issuer.example/a?code=secret", "frustplay");
        let rendered = format!("{custom:?}");
        assert_eq!(
            rendered,
            "AuthSessionRequest { url_len: 36, callback: CustomScheme(\"frustplay\"), ephemeral: false }"
        );
        assert!(!rendered.contains("secret"));

        let https = AuthSessionRequest::https(
            "https://login.example.com/a?state=secret",
            "app.example.com",
            "/cb",
        );
        let rendered = format!("{https:?}");
        assert_eq!(
            rendered,
            "AuthSessionRequest { url_len: 40, callback: Https { host: \"app.example.com\", path: \"/cb\" }, ephemeral: false }"
        );
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("login"));
    }

    /// A custom-scheme request is validated by exactly the scheme rule
    /// table, through `validate`.
    #[test]
    fn validate_applies_the_custom_scheme_rules() {
        assert_eq!(validate(&valid_request()), Ok(()));
        for scheme in ["Loop", "1abc", "", "https", "a b", "my:app"] {
            assert_eq!(
                validate(&AuthSessionRequest::custom_scheme(
                    "https://issuer.example/a",
                    scheme
                )),
                Err(AuthSessionError::InvalidUrl),
                "{scheme:?}"
            );
        }
        // The authorization URL rule applies to either form.
        assert_eq!(
            validate(&AuthSessionRequest::custom_scheme(
                "http://issuer.example/a",
                "frustplay"
            )),
            Err(AuthSessionError::InvalidUrl)
        );
    }

    #[test]
    fn https_callback_host_rules() {
        let label_63 = "a".repeat(63);
        let label_64 = "a".repeat(64);
        // 4 labels of 63 bytes plus 3 dots: 255 bytes.
        let too_long = [label_63.as_str(); 4].join(".");
        let longest = format!("{}.{}.{}.{}", label_63, label_63, label_63, "a".repeat(61));
        assert_eq!(longest.len(), 253);
        for host in [
            "example.com",
            "app.example.com",
            "App.Example.COM",
            "my-app.example.com",
            "localhost",
            "a1.example.com",
            label_63.as_str(),
            longest.as_str(),
        ] {
            assert_eq!(
                validate_https_host(host),
                Ok(()),
                "{host:?} should be accepted"
            );
        }
        for host in [
            "",
            "https://example.com",
            "example.com:443",
            "user@example.com",
            "example.com/cb",
            "example.com?x",
            "example.com#x",
            "exa mple.com",
            "exämple.com",
            "example_app.com",
            ".example.com",
            "example.com.",
            "example..com",
            "-app.example.com",
            "app-.example.com",
            "[::1]",
            label_64.as_str(),
            too_long.as_str(),
        ] {
            assert_eq!(
                validate_https_host(host),
                Err(AuthSessionError::InvalidUrl),
                "{host:?} should be rejected"
            );
        }
    }

    #[test]
    fn https_callback_path_rules() {
        for path in [
            // Leading slash, letters and digits.
            "/",
            "/cb",
            "/oauth/callback",
            "/a/b/c.html",
            "/Case",
            "/v2/0123456789",
            // Unreserved punctuation, dots inside a segment included.
            "/a-b.c_d~e",
            "/.well-known/cb",
            "/a/.../b",
            "/a..b",
            // Every sub-delim.
            "/!$&'()*+,;=",
            // `:` and `@`.
            "/user@host:8080",
            // Empty segments and a trailing slash.
            "/a//b",
            "/cb/",
            // Percent-escapes, either hex case.
            "/a%20b",
            "/%2F%2f",
            "/%aF%Fa%09",
        ] {
            assert_eq!(
                validate_https_path(path),
                Ok(()),
                "{path:?} should be accepted"
            );
        }
        for path in [
            // No leading slash.
            "",
            "cb",
            "oauth/callback",
            // Query and fragment delimiters.
            "/cb?x=1",
            "/cb#frag",
            "/?",
            "/#",
            // Bytes outside the RFC 3986 path set.
            "/a b",
            "/caf\u{e9}",
            "/a\tb",
            "/a\"b",
            "/a<b>",
            "/a\\b",
            "/a[b]",
            "/a^b",
            "/a`b",
            "/a{b}",
            "/a|b",
            "/a\u{7f}",
            // Broken percent-escapes.
            "/%",
            "/a%",
            "/a%2",
            "/a%zz",
            "/a%2G",
            "/a%%20",
            // `.` and `..` segments.
            "/.",
            "/..",
            "/./cb",
            "/../cb",
            "/a/./b",
            "/a/../b",
            "/a/.",
            "/a/..",
            "/a/../",
        ] {
            assert_eq!(
                validate_https_path(path),
                Err(AuthSessionError::InvalidUrl),
                "{path:?} should be rejected"
            );
        }
    }

    /// `validate` routes an https request through the host and path rules
    /// (and the authorization URL rule), every failure `InvalidUrl`.
    #[test]
    fn validate_applies_the_https_rules() {
        assert_eq!(validate(&https_request()), Ok(()));
        for (host, path) in [
            ("", "/cb"),
            ("example.com:8443", "/cb"),
            ("user@example.com", "/cb"),
            ("https://example.com", "/cb"),
            ("example.com", ""),
            ("example.com", "cb"),
            ("example.com", "/cb?x"),
            ("example.com", "/cb#x"),
            ("example.com", "/c b"),
            ("example.com", "/cb%"),
            ("example.com", "/../cb"),
        ] {
            assert_eq!(
                validate(&AuthSessionRequest::https(
                    "https://login.example.com/a",
                    host,
                    path
                )),
                Err(AuthSessionError::InvalidUrl),
                "{host:?} {path:?}"
            );
        }
        assert_eq!(
            validate(&AuthSessionRequest::https(
                "http://login.example.com/a",
                "example.com",
                "/cb"
            )),
            Err(AuthSessionError::InvalidUrl)
        );
    }

    /// The https matcher: scheme and host case-insensitive, path exact,
    /// everything after the path ignored.
    #[test]
    fn https_callback_matching() {
        let matches = |url| https_callback_matches(url, "app.example.com", "/oauth/callback");
        for url in [
            "https://app.example.com/oauth/callback",
            "HTTPS://APP.Example.com/oauth/callback",
            "https://app.example.com/oauth/callback?code=x&state=y",
            "https://app.example.com/oauth/callback#code=x",
            "https://app.example.com/oauth/callback?code=x#frag",
        ] {
            assert!(matches(url), "{url:?} should match");
        }
        for url in [
            "http://app.example.com/oauth/callback",
            "frustplay://app.example.com/oauth/callback",
            "https:app.example.com/oauth/callback",
            "https://evil.example.com/oauth/callback",
            "https://app.example.com.evil.example/oauth/callback",
            "https://app.example.com:443/oauth/callback",
            "https://user@app.example.com/oauth/callback",
            "https://app.example.com/oauth/callback/",
            "https://app.example.com/OAuth/callback",
            "https://app.example.com/oauth/callbackx",
            "https://app.example.com/oauth",
            "https://app.example.com",
            "https://app.example.com?code=x",
            "https://",
            "app.example.com/oauth/callback",
        ] {
            assert!(!matches(url), "{url:?} should not match");
        }
    }

    /// A matching https callback completes the session with the URL exactly
    /// as delivered, query and fragment untouched.
    #[test]
    fn a_matching_https_callback_completes_with_query_and_fragment_kept() {
        with_clean_active(|| {
            let (generation, mut fut) = live_session_for(AuthCallback::Https {
                host: "app.example.com".to_string(),
                path: "/oauth/callback".to_string(),
            });
            let url = "https://App.Example.com/oauth/callback?code=x&state=y#frag";
            resolve(
                generation,
                Ok(AuthSessionOutcome::Callback(url.to_string())),
            );
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Ok(AuthSessionOutcome::Callback(url.to_string())))
            );
            assert!(lock_active().is_none());
        });
    }

    /// A wrong scheme, host or path on an https session is the Platform
    /// error with the https constant, never naming the URL.
    #[test]
    fn a_mismatched_https_callback_is_refused() {
        for url in [
            "http://app.example.com/oauth/callback?code=secret",
            "frustplay://app.example.com/oauth/callback?code=secret",
            "https://other.example.com/oauth/callback?code=secret",
            "https://app.example.com/other?code=secret",
        ] {
            with_clean_active(|| {
                let (generation, mut fut) = live_session_for(AuthCallback::Https {
                    host: "app.example.com".to_string(),
                    path: "/oauth/callback".to_string(),
                });
                resolve(
                    generation,
                    Ok(AuthSessionOutcome::Callback(url.to_string())),
                );
                match poll_once(&mut fut) {
                    Poll::Ready(Err(AuthSessionError::Platform(message))) => {
                        assert_eq!(
                            message,
                            "callback URL does not match the requested https host and path"
                        );
                        assert!(!message.contains("secret"));
                        assert!(!message.contains("example"));
                    }
                    other => panic!("{url:?}: expected Ready(Err(Platform(..))), got {other:?}"),
                }
            });
        }
    }

    /// An empty callback on an https session is the same no-URL error as on
    /// a custom-scheme one.
    #[test]
    fn an_https_callback_with_no_url_is_refused() {
        with_clean_active(|| {
            let (generation, mut fut) = live_session_for(AuthCallback::Https {
                host: "app.example.com".to_string(),
                path: "/cb".to_string(),
            });
            resolve(generation, Ok(AuthSessionOutcome::Callback(String::new())));
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Err(AuthSessionError::Platform(
                    CALLBACK_WITHOUT_URL.to_string()
                )))
            );
        });
    }

    /// Where no backend can catch an https redirect (neither Apple nor
    /// Android), a valid https request is refused on first poll with
    /// `HttpsCallbackUnsupported`, the slot is never claimed, the cancel
    /// handle is inert, and a following custom-scheme start is not `Busy`.
    /// Not built for Apple targets, where a system with the callback API
    /// would accept the request and queue a real session, nor for Android,
    /// where the real predicate asks the JVM: those https paths are covered at
    /// the device gate only.
    #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
    #[test]
    fn an_https_request_is_refused_without_claiming_the_slot() {
        with_clean_active(|| {
            let (mut fut, handle) = AuthSession::start_with_cancel(https_request());
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Err(AuthSessionError::HttpsCallbackUnsupported))
            );
            assert!(handle.generation.is_none(), "the handle must be inert");
            assert!(lock_active().is_none(), "the slot must never be claimed");

            let mut next = AuthSession::start(valid_request());
            let next = poll_once(&mut next);
            assert_ne!(next, Poll::Ready(Err(AuthSessionError::Busy)));
            assert_ne!(
                next,
                Poll::Ready(Err(AuthSessionError::HttpsCallbackUnsupported)),
                "a custom-scheme request takes the unchanged path"
            );
        });
    }

    /// With a session live, an https request is refused (where no backend can
    /// catch one) before the Busy check, and the live session is untouched.
    /// Not built for Apple or Android targets, for the reason on the test
    /// above.
    #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
    #[test]
    fn an_https_refusal_leaves_a_live_session_alone() {
        with_clean_active(|| {
            let (generation, mut live_fut) = live_session("frustplay");
            let mut fut = AuthSession::start(https_request());
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Err(AuthSessionError::HttpsCallbackUnsupported))
            );
            assert_eq!(poll_once(&mut live_fut), Poll::Pending);
            assert_eq!(
                lock_active().as_ref().map(|live| live.generation),
                Some(generation)
            );
        });
    }

    /// The https decision on every target, Apple included, through stand-in
    /// backends: a backend that cannot catch one is never called and the
    /// slot is never claimed; one that can is handed the request, and the
    /// slot is claimed for its host and path.
    #[test]
    fn the_backend_predicate_alone_decides_an_https_request() {
        with_clean_active(|| {
            let mut reached = false;
            let (mut refused, generation) = StartFuture::begin_with(
                https_request(),
                || false,
                |_req, _token| {
                    reached = true;
                    Ok(())
                },
            );
            assert_eq!(
                poll_once(&mut refused),
                Poll::Ready(Err(AuthSessionError::HttpsCallbackUnsupported))
            );
            assert!(generation.is_none());
            assert!(!reached, "a refused request must never reach the backend");
            assert!(lock_active().is_none());

            let mut handed = None;
            let (mut accepted, generation) = StartFuture::begin_with(
                https_request(),
                || true,
                |req, _token| {
                    handed = Some(req.callback);
                    Ok(())
                },
            );
            assert_eq!(poll_once(&mut accepted), Poll::Pending);
            assert!(generation.is_some());
            let expected = AuthCallback::Https {
                host: "app.example.com".to_string(),
                path: "/oauth/callback".to_string(),
            };
            assert_eq!(handed, Some(expected.clone()));
            assert_eq!(
                lock_active().as_ref().map(|live| &live.callback),
                Some(&LiveCallback::Request(expected))
            );
        });
    }

    /// An invalid https request is `InvalidUrl`, not the unsupported
    /// refusal: validation runs first.
    #[test]
    fn an_invalid_https_request_is_invalid_url_first() {
        with_clean_active(|| {
            let mut fut = AuthSession::start(AuthSessionRequest::https(
                "https://login.example.com/a",
                "example.com:443",
                "/cb",
            ));
            assert_eq!(
                poll_once(&mut fut),
                Poll::Ready(Err(AuthSessionError::InvalidUrl))
            );
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
