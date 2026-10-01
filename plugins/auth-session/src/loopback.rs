//! The desktop RFC 8252 loopback-redirect backend — [`LoopbackSession`].
//!
//! Desktop Linux and Windows ship no in-app browser-tab API (see the
//! `unsupported` module), and a macOS app may prefer the user's own default
//! browser over `ASWebAuthenticationSession`. RFC 8252 §7.3 ("Loopback
//! Interface Redirection") covers exactly that case: the app listens on an
//! ephemeral port of the loopback interface, registers
//! `http://127.0.0.1:<port>/<path>` as the redirect URI, opens the
//! authorization URL in the system browser, and reads the authorization
//! response off the one HTTP request the browser then makes to that port.
//! This module is that listener, so an app never hand-rolls a
//! security-sensitive HTTP server of its own.
//!
//! # Lifecycle
//!
//! 1. [`LoopbackSession::bind`] validates the [`LoopbackOptions`], claims the
//!    crate's one live-session slot (the same slot
//!    [`crate::AuthSession::start`] claims — a bound loopback session makes a
//!    custom-scheme session [`AuthSessionError::Busy`] and vice versa), and
//!    binds a listener on `127.0.0.1:0` — the literal IPv4 loopback address,
//!    never `localhost`, `0.0.0.0` or `::`.
//!    [`LoopbackSession::redirect_uri`] is now known, so the caller can put
//!    it into its authorization URL (and later into its token request).
//! 2. [`LoopbackSession::start`] re-validates the authorization URL with the
//!    crate's `https`-only rule, spawns one `std::thread` that owns the
//!    listener and runs the accept loop, then opens the URL in the system
//!    browser through `frust-url-launcher`. The future it returns resolves to
//!    [`AuthSessionOutcome::Callback`] carrying the full
//!    `http://127.0.0.1:<port><path>?<query>` request URL, query intact
//!    byte-for-byte; to [`AuthSessionOutcome::Cancelled`] after
//!    [`LoopbackCancel::cancel`]; or to an error —
//!    [`AuthSessionError::TimedOut`] once [`LoopbackOptions::timeout`]
//!    elapses with no matching request.
//! 3. The listener socket closes on **every** exit path: success, cancel,
//!    timeout, an accept failure, the caller dropping the future (its `Drop`
//!    raises a shutdown flag the accept loop polls), or the caller dropping
//!    an unstarted [`LoopbackSession`]. A cancel, dropping the future and the
//!    timeout all take effect within about one poll interval (25 ms), even
//!    while a connection is being served — the one exception is a response
//!    write already in progress, bounded by [`CONNECTION_IO_TIMEOUT`].
//!    Exactly one callback is ever accepted — the loop exits right after it.
//!
//! # What the listener answers
//!
//! Connections are served one at a time. Each gets 2 s to deliver its
//! request head (at most 8 KiB up to the blank line); a slow, oversized or
//! malformed request is answered `400` and the listener **keeps waiting**,
//! so a stray local client cannot end the session. A request is only a
//! callback when all of these hold, checked in this order:
//!
//! - the request line is `METHOD SP TARGET SP HTTP/1.0|HTTP/1.1`, and the
//!   target starts with `/` and holds only printable ASCII with no `#`
//!   (else `400`);
//! - exactly one `Host` header is present and it equals `127.0.0.1:<port>`
//!   (ASCII-case-insensitively) — this is the DNS-rebinding defence: a web
//!   page that resolves its own hostname to `127.0.0.1` still sends its own
//!   hostname as `Host` (else `400`);
//! - the method is `GET` — `HEAD`, `POST` and everything else are `404`;
//! - the target's path (the bytes before the first `?`) equals the reserved
//!   path exactly (else `404`);
//! - `Sec-Fetch-Mode`, if present, is `navigate`, and `Sec-Fetch-Dest`, if
//!   present, is `document` (else `404`) — see the *Security* section's
//!   Fetch Metadata rule;
//! - the reconstructed callback URL passes the crate's shared `url`
//!   validator (else `400`).
//!
//! Every response is a static `HTTP/1.1` page with `Cache-Control:
//! no-store`, `Content-Security-Policy: default-src 'none'`,
//! `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff` and
//! `Connection: close`. No byte of the request is ever echoed into a
//! response, and the success page carries no script.
//!
//! # Redaction
//!
//! No `Debug`/`Display` impl or error string in this module prints the
//! callback URL or its query (the crate doc's *The URL never appears in a
//! `Display`/`Debug` string*): [`LoopbackSession`]'s `Debug` prints only the
//! port and the reserved path, and failures report an
//! [`std::io::ErrorKind`] rather than an OS message.
//!
//! # Security
//!
//! Any local process can connect to the port while the session is live, and
//! the authorization response is plain `http` on the loopback interface.
//! The crate doc's PKCE + `state` requirement therefore binds here exactly as
//! it does for a custom-scheme callback: a [`AuthSessionOutcome::Callback`]
//! proves only that *some* local client sent a well-formed request to the
//! reserved path.
//!
//! A same-user local process can still delay the real redirect by at most
//! one request-head budget ([`CONNECTION_IO_TIMEOUT`], 2 s) per silent
//! connection it opens, by holding the listener through each one's cutoff —
//! cancel, a dropped future and [`LoopbackOptions::timeout`] all still fire
//! on time regardless, within about one poll interval, because they
//! interrupt a connection that is mid-read rather than waiting for it to
//! finish.
//!
//! **Fetch Metadata.** Any web origin open in the browser can end a loopback
//! session with a no-cors `GET http://127.0.0.1:<port>/<path>` request (a
//! port scan inside the timeout) — no PKCE/`state` check runs until the
//! caller sees a callback, so this is defence in depth, not the crate's only
//! defence. A browser tags such a cross-origin, non-navigation request with
//! `Sec-Fetch-Mode`/`Sec-Fetch-Dest`; [`Server::check`] answers `404` (and
//! keeps waiting) when `Sec-Fetch-Mode` is present and is not `navigate`, or
//! `Sec-Fetch-Dest` is present and is not `document` — the same family as a
//! wrong path, since a well-formed request from a forbidden context has
//! nothing to reveal. Either header's absence (curl, older browsers that
//! predate Fetch Metadata, the test harness) leaves the request allowed
//! unchanged; a duplicated `Sec-Fetch-Mode`/`Sec-Fetch-Dest` header is `400`,
//! like a duplicated `Host`.

use std::fmt;
use std::future::Future;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    AuthSessionError, AuthSessionOutcome, Live, lock_active, next_generation, oneshot,
    release_if_live, resolve, url, validate_https_url,
};

/// The [`LoopbackOptions::timeout`] [`LoopbackOptions::new`] fills in: five
/// minutes, long enough for a sign-in that includes a password-manager
/// unlock or a second-factor prompt.
pub const DEFAULT_LOOPBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// The longest [`LoopbackOptions::timeout`] [`LoopbackSession::bind`]
/// accepts — one hour. A listener left open longer than any realistic
/// sign-in is attack surface with no user benefit.
const MAX_LOOPBACK_TIMEOUT: Duration = Duration::from_secs(3600);

/// The longest reserved path [`LoopbackSession::bind`] accepts, in bytes.
const MAX_PATH_LEN: usize = 256;

/// How long the accept loop sleeps when no connection is pending — the
/// upper bound on how late it notices a cancel, a timeout or a dropped
/// future.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The read deadline for a whole request head, and the write timeout for a
/// response — a client that has not finished its request head within it is
/// answered `400` and cut off, so one slow client cannot hold the
/// (sequential) listener for long.
const CONNECTION_IO_TIMEOUT: Duration = Duration::from_secs(2);

/// The most bytes read for one request head, terminator included.
const MAX_REQUEST_HEAD: usize = 8192;

/// How long a connection is drained after its response (and the write-side
/// shutdown) before it is closed. Closing a socket with unread request bytes
/// queued makes the OS send a reset, which can discard the response before
/// the client reads it; a short, bounded drain avoids that without letting a
/// client hold the listener.
const LINGER: Duration = Duration::from_millis(250);

/// The most bytes [`LINGER`] drains before closing anyway.
const LINGER_MAX_BYTES: usize = 64 * 1024;

/// The `200` page. Static, script-free, and carrying no request-derived
/// byte.
const SUCCESS_BODY: &str = "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>Sign-in complete</title></head><body><p>Sign-in complete. You can close this tab and \
return to the app.</p></body></html>\n";

/// The `404` body.
const NOT_FOUND_BODY: &str = "Not found\n";

/// The `400` body.
const BAD_REQUEST_BODY: &str = "Bad request\n";

/// The [`AuthSessionError::Platform`] message for an accept-loop thread that
/// panicked. Names no URL.
const LISTENER_PANICKED: &str = "loopback listener stopped unexpectedly";

/// Options for [`LoopbackSession::bind`].
#[derive(Clone, Debug)]
pub struct LoopbackOptions {
    /// The reserved redirect path, e.g. `/callback`. Must start with `/`, be
    /// 2-256 bytes, hold only ASCII alphanumerics and `-` `.` `_` `~` `/`,
    /// and contain no empty (`//`), `.` or `..` segment — so no `?`, `#` or
    /// `%`. A trailing `/` is allowed; `/` alone is not.
    pub path: String,
    /// How long the started session waits for the redirect before resolving
    /// [`AuthSessionError::TimedOut`]. Must be non-zero and at most one
    /// hour.
    pub timeout: Duration,
}

impl LoopbackOptions {
    /// Options for `path` with [`DEFAULT_LOOPBACK_TIMEOUT`].
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            timeout: DEFAULT_LOOPBACK_TIMEOUT,
        }
    }
}

/// The flags the accept-loop thread polls, shared with the caller's future
/// and every [`LoopbackCancel`].
#[derive(Default)]
struct Flags {
    /// Set by [`LoopbackCancel::cancel`]: resolve `Cancelled` and stop.
    cancel: AtomicBool,
    /// Set when the caller's future is dropped (or the browser failed to
    /// open): stop silently — nothing is waiting for a result any more.
    shutdown: AtomicBool,
}

/// A bound, not yet started RFC 8252 loopback redirect listener — see the
/// module doc for the whole lifecycle.
///
/// Holds the crate's one live-session slot from [`Self::bind`] until the
/// session resolves or is dropped. Dropping an unstarted session closes its
/// socket and frees the slot. Not `Clone`; `Debug` prints only the port and
/// the reserved path.
pub struct LoopbackSession {
    /// The bound `127.0.0.1:<port>` listener.
    listener: TcpListener,
    /// The future half of the live session's channel. Dropping it (with an
    /// unstarted session) releases the slot via `crate::release_if_live`.
    receiver: oneshot::Receiver,
    /// The live session's generation — every resolution goes through
    /// `crate::resolve` with it.
    generation: u64,
    /// The listener's port.
    port: u16,
    /// The validated reserved path.
    path: String,
    /// `http://127.0.0.1:<port><path>`.
    redirect_uri: String,
    /// The validated [`LoopbackOptions::timeout`].
    timeout: Duration,
    /// Shared with the accept loop, the future and every [`LoopbackCancel`].
    flags: Arc<Flags>,
}

impl fmt::Debug for LoopbackSession {
    /// Prints the port and the reserved path only — neither is secret, and
    /// nothing request-derived is held here.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoopbackSession")
            .field("port", &self.port)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl LoopbackSession {
    /// Whether this build target has the loopback backend: desktop Linux,
    /// Windows and macOS. A compile-time answer, not a runtime probe.
    pub fn is_supported() -> bool {
        cfg!(any(
            target_os = "linux",
            target_os = "windows",
            target_os = "macos"
        ))
    }

    /// Validate `opts`, claim the live-session slot and bind
    /// `127.0.0.1:0`.
    ///
    /// # Errors
    /// [`AuthSessionError::InvalidUrl`] for a path or timeout outside
    /// [`LoopbackOptions`]' rules; [`AuthSessionError::NoHandler`] on a
    /// target [`Self::is_supported`] rejects (no socket is opened);
    /// [`AuthSessionError::Busy`] while another session — loopback or
    /// [`crate::AuthSession`] — is live; [`AuthSessionError::Platform`]
    /// naming only an [`std::io::ErrorKind`] if the socket cannot be bound.
    pub fn bind(opts: LoopbackOptions) -> Result<Self, AuthSessionError> {
        validate_path(&opts.path)?;
        if opts.timeout.is_zero() || opts.timeout > MAX_LOOPBACK_TIMEOUT {
            return Err(AuthSessionError::InvalidUrl);
        }
        if !Self::is_supported() {
            return Err(AuthSessionError::NoHandler);
        }

        let (generation, receiver) = {
            let mut active = lock_active();
            if active.is_some() {
                return Err(AuthSessionError::Busy);
            }
            let generation = next_generation();
            let (sender, receiver) = oneshot::channel(generation);
            // The `http` scheme makes `crate::checked_callback` accept only
            // an `http://` callback for this session.
            *active = Some(Live {
                generation,
                callback_scheme: "http".into(),
                sender,
            });
            (generation, receiver)
        };

        let bound = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).and_then(|listener| {
            let port = listener.local_addr()?.port();
            Ok((listener, port))
        });
        let (listener, port) = match bound {
            Ok(bound) => bound,
            Err(err) => {
                release_if_live(generation);
                return Err(AuthSessionError::Platform(format!(
                    "loopback bind failed: {:?}",
                    err.kind()
                )));
            }
        };

        Ok(Self {
            listener,
            receiver,
            generation,
            port,
            redirect_uri: format!("http://127.0.0.1:{port}{}", opts.path),
            path: opts.path,
            timeout: opts.timeout,
            flags: Arc::new(Flags::default()),
        })
    }

    /// The redirect URI to register with the identity provider and send in
    /// the authorization and token requests — exactly
    /// `http://127.0.0.1:<port><path>`.
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// A handle that cancels this session from any thread, before or after
    /// [`Self::start`].
    pub fn cancel_handle(&self) -> LoopbackCancel {
        LoopbackCancel {
            flags: Arc::clone(&self.flags),
        }
    }

    /// Start listening and open `authorization_url` in the system browser.
    ///
    /// The returned future follows the crate doc's *The awaitable-future
    /// contract*: poll it from any executor; it keeps returning
    /// [`Poll::Pending`] after it has completed; dropping it stops the
    /// listener (within about one poll interval — 25 ms — even while a
    /// connection is being served; the one exception is a response write
    /// already in progress, bounded by the 2 s connection I/O timeout) and
    /// frees the slot.
    ///
    /// # Errors
    /// The future resolves [`AuthSessionError::InvalidUrl`] when
    /// `authorization_url` is not an absolute `https` URL (the socket is
    /// closed without the browser being opened); the URL launcher's
    /// [`AuthSessionError::NoHandler`]/
    /// [`AuthSessionError::PlatformNotInitialized`]/
    /// [`AuthSessionError::Platform`] when the browser cannot be opened;
    /// [`AuthSessionError::TimedOut`] when no callback arrives within
    /// [`LoopbackOptions::timeout`]; [`AuthSessionError::Platform`] naming
    /// an [`std::io::ErrorKind`] when the listener itself fails.
    pub fn start(
        self,
        authorization_url: &str,
    ) -> impl Future<Output = Result<AuthSessionOutcome, AuthSessionError>> + Send + 'static + use<>
    {
        self.start_with_opener(authorization_url, open_in_browser)
    }

    /// [`Self::start`] with the browser launch injected — the test seam that
    /// keeps host tests from ever launching a browser.
    pub(crate) fn start_with_opener(
        self,
        authorization_url: &str,
        opener: impl FnOnce(&str) -> Result<(), AuthSessionError>,
    ) -> LoopbackFuture {
        let LoopbackSession {
            listener,
            receiver,
            generation,
            port,
            path,
            timeout,
            flags,
            ..
        } = self;
        let future = LoopbackFuture {
            receiver,
            flags: Arc::clone(&flags),
        };

        if let Err(err) = validate_https_url(authorization_url) {
            drop(listener);
            resolve(generation, Err(err));
            return future;
        }
        // Cancelled before it ever started: never open the browser.
        if flags.cancel.load(Ordering::SeqCst) {
            drop(listener);
            resolve(generation, Ok(AuthSessionOutcome::Cancelled));
            return future;
        }

        let server = Server {
            listener,
            generation,
            port,
            path,
            deadline: Instant::now() + timeout,
            flags: Arc::clone(&flags),
        };
        let spawned = thread::Builder::new()
            .name("frust-auth-loopback".into())
            .spawn(move || server.run());
        if let Err(err) = spawned {
            // The closure — and with it the listener — was dropped.
            resolve(
                generation,
                Err(AuthSessionError::Platform(format!(
                    "loopback listener thread failed to start: {:?}",
                    err.kind()
                ))),
            );
            return future;
        }

        if let Err(err) = opener(authorization_url) {
            flags.shutdown.store(true, Ordering::SeqCst);
            resolve(generation, Err(err));
        }
        future
    }
}

/// Cancels a [`LoopbackSession`] from any thread: a started session resolves
/// [`AuthSessionOutcome::Cancelled`] within about one poll interval (25 ms),
/// even while a connection is being served — the one exception is a
/// response write already in progress, bounded by the 2 s connection I/O
/// timeout — and closes its socket; a session cancelled before
/// [`LoopbackSession::start`] resolves `Cancelled` from `start` without
/// opening the browser. A no-op once the session has resolved.
#[derive(Clone)]
pub struct LoopbackCancel {
    flags: Arc<Flags>,
}

impl LoopbackCancel {
    /// Request cancellation — see the type's doc.
    pub fn cancel(&self) {
        self.flags.cancel.store(true, Ordering::SeqCst);
    }
}

impl fmt::Debug for LoopbackCancel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoopbackCancel").finish_non_exhaustive()
    }
}

/// The future [`LoopbackSession::start`] returns: the live session's
/// [`oneshot::Receiver`] plus the flags its `Drop` uses to stop the accept
/// loop.
pub(crate) struct LoopbackFuture {
    /// Resolved through `crate::resolve` only. Polls `Pending` forever once
    /// its value has been taken, like `crate::StartFuture`.
    receiver: oneshot::Receiver,
    flags: Arc<Flags>,
}

impl Future for LoopbackFuture {
    type Output = Result<AuthSessionOutcome, AuthSessionError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().receiver).poll(cx)
    }
}

impl Drop for LoopbackFuture {
    /// Stop the accept loop. The `receiver` field drops right after this,
    /// releasing the slot if the session is still live.
    fn drop(&mut self) {
        self.flags.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Open `url` in the system browser through `frust-url-launcher`.
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn open_in_browser(url: &str) -> Result<(), AuthSessionError> {
    frust_url_launcher::UrlLauncher::open_external(url).map_err(map_launcher_error)
}

/// No loopback backend on this target — unreachable in practice, because
/// [`LoopbackSession::bind`] already refuses with the same error.
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn open_in_browser(_url: &str) -> Result<(), AuthSessionError> {
    Err(AuthSessionError::NoHandler)
}

/// Map a URL-launcher failure onto this crate's error, variant for variant.
/// The launcher's error enum is `#[non_exhaustive]`, so a future variant
/// maps to a fixed, URL-free `Platform` message.
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn map_launcher_error(err: frust_url_launcher::UrlLauncherError) -> AuthSessionError {
    use frust_url_launcher::UrlLauncherError;
    match err {
        UrlLauncherError::InvalidUrl => AuthSessionError::InvalidUrl,
        UrlLauncherError::NoHandler => AuthSessionError::NoHandler,
        UrlLauncherError::PlatformNotInitialized => AuthSessionError::PlatformNotInitialized,
        UrlLauncherError::Platform(message) => AuthSessionError::Platform(message),
        _ => AuthSessionError::Platform("url launcher failed".to_string()),
    }
}

/// Validate a [`LoopbackOptions::path`] against the rules on that field.
fn validate_path(path: &str) -> Result<(), AuthSessionError> {
    let bytes = path.as_bytes();
    if bytes.len() < 2 || bytes.len() > MAX_PATH_LEN || bytes[0] != b'/' {
        return Err(AuthSessionError::InvalidUrl);
    }
    if !bytes.iter().all(|&byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/')
    }) {
        return Err(AuthSessionError::InvalidUrl);
    }
    let segments: Vec<&str> = path[1..].split('/').collect();
    let last = segments.len() - 1;
    for (index, segment) in segments.iter().enumerate() {
        // An empty segment is a `//`, except the one a trailing `/` leaves.
        let empty_inside = segment.is_empty() && index != last;
        if empty_inside || *segment == "." || *segment == ".." {
            return Err(AuthSessionError::InvalidUrl);
        }
    }
    Ok(())
}

/// What one served connection means for the accept loop.
enum Served {
    /// Answered `400`/`404` (or dropped) — keep accepting.
    KeepWaiting,
    /// Closed without a response because the accept loop's own shutdown,
    /// cancel or deadline condition fired while this connection was still
    /// being read or drained — nothing to say, so the loop's own
    /// top-of-loop checks resolve or exit on the very next pass.
    Interrupted,
    /// The callback was resolved (or the session was abandoned meanwhile) —
    /// stop.
    Done,
}

/// The response statuses this listener ever sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Ok,
    BadRequest,
    NotFound,
}

/// The accept-loop thread's state. Owns the listener, so the socket closes
/// whenever [`Server::run`] returns or unwinds.
struct Server {
    listener: TcpListener,
    generation: u64,
    port: u16,
    path: String,
    /// The absolute instant [`LoopbackOptions::timeout`] elapses — computed
    /// once when the thread is spawned rather than re-derived from an
    /// elapsed-since-start duration, so [`Self::interrupted`] can read it
    /// from `read_head`/`linger` as well as the accept loop itself.
    deadline: Instant,
    flags: Arc<Flags>,
}

impl Server {
    /// The thread body: the accept loop, with a panic turned into a
    /// URL-free `Platform` resolution rather than a slot left live.
    fn run(self) {
        let generation = self.generation;
        if panic::catch_unwind(AssertUnwindSafe(move || self.accept_loop())).is_err() {
            resolve(
                generation,
                Err(AuthSessionError::Platform(LISTENER_PANICKED.to_string())),
            );
        }
    }

    /// Poll for connections until a callback, a cancel, the timeout, a
    /// shutdown or a listener failure (the module doc's *Lifecycle*).
    fn accept_loop(self) {
        if let Err(err) = self.listener.set_nonblocking(true) {
            resolve(self.generation, Err(listener_error(&err)));
            return;
        }
        loop {
            if self.flags.shutdown.load(Ordering::SeqCst) {
                // The caller dropped the future: its receiver already
                // released the slot, and nothing is waiting for a result.
                return;
            }
            if self.flags.cancel.load(Ordering::SeqCst) {
                resolve(self.generation, Ok(AuthSessionOutcome::Cancelled));
                return;
            }
            if Instant::now() >= self.deadline {
                resolve(self.generation, Err(AuthSessionError::TimedOut));
                return;
            }
            match self.listener.accept() {
                Ok((stream, _peer)) => match self.serve(stream) {
                    Served::Done => return,
                    // `Interrupted` answered nothing and closed already;
                    // the checks above resolve or exit on the very next
                    // pass rather than this one re-deriving which flag won.
                    Served::KeepWaiting | Served::Interrupted => {}
                },
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL_INTERVAL),
                // A signal, or a client that gave up before it was
                // accepted: a per-connection event, not a listener failure.
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::Interrupted
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::ConnectionReset
                    ) => {}
                Err(err) => {
                    resolve(self.generation, Err(listener_error(&err)));
                    return;
                }
            }
        }
    }

    /// Whether the accept loop's own exit conditions have fired — shutdown,
    /// cancel or the session deadline. `read_head` and `linger` check this
    /// once per [`POLL_INTERVAL`] pass so a cancel, a dropped future or
    /// [`LoopbackOptions::timeout`] interrupts a connection that is
    /// mid-read/mid-drain, rather than waiting out that connection's own I/O
    /// timeout first. All three conditions are monotonic (`cancel`/
    /// `shutdown` only ever go false→true; `deadline` only moves closer),
    /// so a caller that sees `true` here can act on it immediately — it
    /// never flips back to `false`.
    fn interrupted(&self) -> bool {
        self.flags.shutdown.load(Ordering::SeqCst)
            || self.flags.cancel.load(Ordering::SeqCst)
            || Instant::now() >= self.deadline
    }

    /// Serve one connection (the module doc's *What the listener answers*).
    fn serve(&self, mut stream: TcpStream) -> Served {
        if stream.set_nonblocking(false).is_err()
            || stream
                .set_write_timeout(Some(CONNECTION_IO_TIMEOUT))
                .is_err()
        {
            return Served::KeepWaiting;
        }
        let head = match self.read_head(&mut stream) {
            ReadOutcome::Head(head) => head,
            ReadOutcome::BadRequest => return self.reject(stream, Status::BadRequest),
            ReadOutcome::Interrupted => return Served::Interrupted,
        };
        let target = match self.check(&head) {
            Ok(target) => target,
            Err(status) => return self.reject(stream, status),
        };
        let callback = format!("http://127.0.0.1:{}{target}", self.port);
        if url::validate(&callback).is_err() {
            return self.reject(stream, Status::BadRequest);
        }
        if self.flags.shutdown.load(Ordering::SeqCst) {
            // Abandoned while this request was being read: answer nothing.
            return Served::Done;
        }
        respond(&mut stream, Status::Ok);
        resolve(self.generation, Ok(AuthSessionOutcome::Callback(callback)));
        self.linger(stream);
        Served::Done
    }

    /// Read one request head — everything before the first `\r\n\r\n` —
    /// within [`head_budget`] and [`MAX_REQUEST_HEAD`] bytes, in passes no
    /// longer than [`POLL_INTERVAL`] so [`Self::interrupted`] is checked
    /// well before a silent connection could otherwise hold the listener
    /// for the whole head budget. [`ReadOutcome::BadRequest`] on a timeout,
    /// an oversized head, EOF or any read error — the existing `400`
    /// contract, unchanged; [`ReadOutcome::Interrupted`] only when
    /// [`Self::interrupted`] fires first.
    fn read_head(&self, stream: &mut TcpStream) -> ReadOutcome {
        let head_deadline = Instant::now() + head_budget();
        let mut head = Vec::with_capacity(1024);
        let mut chunk = [0u8; 1024];
        loop {
            if self.interrupted() {
                return ReadOutcome::Interrupted;
            }
            let room = MAX_REQUEST_HEAD - head.len();
            let remaining = head_deadline.saturating_duration_since(Instant::now());
            if room == 0 || remaining.is_zero() {
                return ReadOutcome::BadRequest;
            }
            let pass = remaining.min(POLL_INTERVAL);
            if stream.set_read_timeout(Some(pass)).is_err() {
                return ReadOutcome::BadRequest;
            }
            let want = room.min(chunk.len());
            match stream.read(&mut chunk[..want]) {
                Ok(0) => return ReadOutcome::BadRequest,
                Ok(read) => {
                    let scan_from = head.len().saturating_sub(3);
                    head.extend_from_slice(&chunk[..read]);
                    if let Some(end) = head[scan_from..]
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                    {
                        head.truncate(scan_from + end);
                        return ReadOutcome::Head(head);
                    }
                }
                // One poll pass elapsed with no data: loop back to the top,
                // which re-checks `interrupted()` before trying again.
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return ReadOutcome::BadRequest,
            }
        }
    }

    /// Answer a non-matching request and keep waiting.
    fn reject(&self, mut stream: TcpStream, status: Status) -> Served {
        respond(&mut stream, status);
        self.linger(stream);
        Served::KeepWaiting
    }

    /// Drain whatever the client still sends for at most [`LINGER`] /
    /// [`LINGER_MAX_BYTES`], then close — see [`LINGER`]. Like
    /// [`Self::read_head`], this reads in [`POLL_INTERVAL`]-sized passes and
    /// checks [`Self::interrupted`] between them, so a cancel, a dropped
    /// future or the session deadline closes a lingering connection
    /// promptly rather than waiting out the whole drain window.
    fn linger(&self, mut stream: TcpStream) {
        let drain_deadline = Instant::now() + LINGER;
        let mut sink = [0u8; 1024];
        let mut drained = 0usize;
        while drained < LINGER_MAX_BYTES {
            if self.interrupted() {
                return;
            }
            let remaining = drain_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            let pass = remaining.min(POLL_INTERVAL);
            if stream.set_read_timeout(Some(pass)).is_err() {
                return;
            }
            match stream.read(&mut sink) {
                Ok(0) => return,
                Ok(read) => drained += read,
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        }
    }

    /// Check a request head, returning the request target of a matching
    /// callback or the status to answer instead.
    fn check<'h>(&self, head: &'h [u8]) -> Result<&'h str, Status> {
        let mut lines = split_lines(head)?.into_iter();
        let request_line = lines.next().ok_or(Status::BadRequest)?;

        let mut parts = request_line.split(|&byte| byte == b' ');
        let (Some(method), Some(target), Some(version), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Status::BadRequest);
        };
        if method.is_empty() || !method.iter().copied().all(is_token_byte) {
            return Err(Status::BadRequest);
        }
        if version != b"HTTP/1.1" && version != b"HTTP/1.0" {
            return Err(Status::BadRequest);
        }
        if target.first() != Some(&b'/')
            || !target
                .iter()
                .all(|&byte| (0x21..=0x7E).contains(&byte) && byte != b'#')
        {
            return Err(Status::BadRequest);
        }

        let mut host = None;
        let mut host_count = 0usize;
        let mut sec_fetch_mode = None;
        let mut sec_fetch_mode_count = 0usize;
        let mut sec_fetch_dest = None;
        let mut sec_fetch_dest_count = 0usize;
        for line in lines {
            let colon = line
                .iter()
                .position(|&byte| byte == b':')
                .ok_or(Status::BadRequest)?;
            let name = &line[..colon];
            // An empty name, or a leading space/tab (obsolete line folding),
            // is malformed.
            if name.is_empty() || !name.iter().copied().all(is_token_byte) {
                return Err(Status::BadRequest);
            }
            if name.eq_ignore_ascii_case(b"host") {
                host_count += 1;
                host = Some(trim_ows(&line[colon + 1..]));
            } else if name.eq_ignore_ascii_case(b"sec-fetch-mode") {
                sec_fetch_mode_count += 1;
                sec_fetch_mode = Some(trim_ows(&line[colon + 1..]));
            } else if name.eq_ignore_ascii_case(b"sec-fetch-dest") {
                sec_fetch_dest_count += 1;
                sec_fetch_dest = Some(trim_ows(&line[colon + 1..]));
            }
        }
        let expected_host = format!("127.0.0.1:{}", self.port);
        match host {
            Some(value)
                if host_count == 1 && value.eq_ignore_ascii_case(expected_host.as_bytes()) => {}
            _ => return Err(Status::BadRequest),
        }
        // A duplicated Fetch Metadata header is malformed, same as a
        // duplicated Host.
        if sec_fetch_mode_count > 1 || sec_fetch_dest_count > 1 {
            return Err(Status::BadRequest);
        }

        if method != b"GET" {
            return Err(Status::NotFound);
        }
        let path_end = target
            .iter()
            .position(|&byte| byte == b'?')
            .unwrap_or(target.len());
        if &target[..path_end] != self.path.as_bytes() {
            return Err(Status::NotFound);
        }
        // Fetch Metadata defence in depth: a no-cors cross-origin request
        // (e.g. a port scan from any open web origin) carries a
        // `Sec-Fetch-Mode` other than `navigate` or a `Sec-Fetch-Dest` other
        // than `document`. A well-formed request from a forbidden context
        // has nothing to reveal, so it is answered the same as a wrong path.
        // Either header's absence (curl, older browsers, the test harness)
        // leaves the request allowed.
        if sec_fetch_mode.is_some_and(|value| !value.eq_ignore_ascii_case(b"navigate"))
            || sec_fetch_dest.is_some_and(|value| !value.eq_ignore_ascii_case(b"document"))
        {
            return Err(Status::NotFound);
        }
        std::str::from_utf8(target).map_err(|_| Status::BadRequest)
    }
}

/// A listener-level I/O failure as a URL-free `Platform` error.
fn listener_error(err: &io::Error) -> AuthSessionError {
    AuthSessionError::Platform(format!("loopback listener failed: {:?}", err.kind()))
}

/// What [`Server::read_head`] found.
enum ReadOutcome {
    /// The full head, terminator stripped.
    Head(Vec<u8>),
    /// Malformed, oversized, EOF, a read error, or the per-connection head
    /// budget ([`head_budget`]) expired — the existing `400` contract,
    /// unchanged.
    BadRequest,
    /// [`Server::interrupted`] fired mid-read — close without a response;
    /// nothing to say, and nothing to gain by finishing the read.
    Interrupted,
}

/// The per-connection head budget [`Server::read_head`] honours —
/// [`CONNECTION_IO_TIMEOUT`] in production. **Test-only seam**: the `live`
/// test module's flood test
/// (`five_silent_clients_do_not_starve_a_later_request`) shrinks this via
/// `TEST_HEAD_BUDGET_MS` to keep five sequential cutoffs out of the test
/// suite's wall-clock budget; every other test — and every non-test build —
/// sees the real [`CONNECTION_IO_TIMEOUT`], since the override defaults to
/// `0` (meaning "unset") and is only ever changed, then reset, inside that
/// one test's body while `serial()` holds the crate's one test-wide lock.
fn head_budget() -> Duration {
    #[cfg(test)]
    {
        let millis = TEST_HEAD_BUDGET_MS.load(Ordering::SeqCst);
        if millis != 0 {
            return Duration::from_millis(millis);
        }
    }
    CONNECTION_IO_TIMEOUT
}

/// [`head_budget`]'s test-only override, in milliseconds; `0` means unset.
#[cfg(test)]
static TEST_HEAD_BUDGET_MS: AtomicU64 = AtomicU64::new(0);

/// Split a request head into its `\r\n`-separated lines. A bare `\r` or `\n`
/// inside a line is malformed (`400`).
fn split_lines(head: &[u8]) -> Result<Vec<&[u8]>, Status> {
    let mut lines = Vec::new();
    let mut rest = head;
    loop {
        match rest.windows(2).position(|window| window == b"\r\n") {
            Some(end) => {
                lines.push(&rest[..end]);
                rest = &rest[end + 2..];
            }
            None => {
                lines.push(rest);
                break;
            }
        }
    }
    if lines
        .iter()
        .any(|line| line.iter().any(|&byte| byte == b'\r' || byte == b'\n'))
    {
        return Err(Status::BadRequest);
    }
    Ok(lines)
}

/// An RFC 9110 `tchar` — the bytes a method or header name may hold.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Trim optional whitespace (spaces and tabs) from both ends of a header
/// value.
fn trim_ows(value: &[u8]) -> &[u8] {
    let is_ows = |byte: &u8| *byte == b' ' || *byte == b'\t';
    let start = value.iter().position(|b| !is_ows(b)).unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|b| !is_ows(b))
        .map_or(start, |end| end + 1);
    &value[start..end]
}

/// The full static response for `status` — headers plus a constant body,
/// never a request-derived byte.
fn response(status: Status) -> String {
    let (code, reason, body) = match status {
        Status::Ok => (200, "OK", SUCCESS_BODY),
        Status::BadRequest => (400, "Bad Request", BAD_REQUEST_BODY),
        Status::NotFound => (404, "Not Found", NOT_FOUND_BODY),
    };
    format!(
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Pragma: no-cache\r\n\
         Content-Security-Policy: default-src 'none'\r\n\
         Referrer-Policy: no-referrer\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )
}

/// Write `status`'s response, flush, and shut the write side down so the
/// client sees the end of the response. Write errors are ignored — the
/// client may already be gone, and nothing about the session depends on it.
fn respond(stream: &mut TcpStream, status: Status) {
    let _ = stream.write_all(response(status).as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_rule_table() {
        for path in [
            "/callback",
            "/a/b-c_d.e~f",
            "/cb/",
            "/A1",
            "/.well-known/cb",
        ] {
            assert!(validate_path(path).is_ok(), "{path:?} should be accepted");
        }
        let long = format!("/{}", "a".repeat(MAX_PATH_LEN));
        let longest = format!("/{}", "a".repeat(MAX_PATH_LEN - 1));
        assert!(validate_path(&longest).is_ok());
        for path in [
            "",
            "/",
            "callback",
            "//",
            "//cb",
            "/a//b",
            "/a//",
            "/.",
            "/..",
            "/a/./b",
            "/a/../b",
            "/a/..",
            "/a?x",
            "/a#x",
            "/a%20",
            "/a b",
            "/é",
            "/a\\b",
            long.as_str(),
        ] {
            assert_eq!(
                validate_path(path),
                Err(AuthSessionError::InvalidUrl),
                "{path:?} should be rejected"
            );
        }
    }

    #[test]
    fn responses_are_static_and_carry_the_security_headers() {
        for status in [Status::Ok, Status::BadRequest, Status::NotFound] {
            let text = response(status);
            for header in [
                "Content-Type: text/html; charset=utf-8\r\n",
                "Cache-Control: no-store\r\n",
                "Pragma: no-cache\r\n",
                "Content-Security-Policy: default-src 'none'\r\n",
                "Referrer-Policy: no-referrer\r\n",
                "X-Content-Type-Options: nosniff\r\n",
                "Connection: close\r\n",
            ] {
                assert!(text.contains(header), "{status:?} lacks {header:?}");
            }
            assert!(!text.contains("<script"));
            let (head, body) = text.split_once("\r\n\r\n").expect("head/body split");
            assert!(head.contains(&format!("Content-Length: {}\r\n", body.len())));
        }
        assert!(response(Status::Ok).starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response(Status::BadRequest).starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(response(Status::NotFound).starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    /// The options half of the invalid-input rule table — rejected before
    /// any slot or socket, on every target.
    #[test]
    fn invalid_options_are_rejected_with_invalid_url() {
        let _guard = serial();
        for timeout in [
            Duration::ZERO,
            MAX_LOOPBACK_TIMEOUT + Duration::from_secs(1),
        ] {
            let opts = LoopbackOptions {
                path: "/callback".to_string(),
                timeout,
            };
            assert_eq!(
                LoopbackSession::bind(opts).unwrap_err(),
                AuthSessionError::InvalidUrl
            );
        }
        for path in ["/", "callback", "/a//b", "/a?b"] {
            assert_eq!(
                LoopbackSession::bind(LoopbackOptions::new(path)).unwrap_err(),
                AuthSessionError::InvalidUrl
            );
        }
        assert!(lock_active().is_none(), "no slot may be claimed");
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    #[test]
    fn bind_reports_no_handler_off_desktop() {
        let _guard = serial();
        assert!(!LoopbackSession::is_supported());
        assert_eq!(
            LoopbackSession::bind(LoopbackOptions::new("/callback")).unwrap_err(),
            AuthSessionError::NoHandler
        );
        assert!(lock_active().is_none());
    }

    /// Serialize with every other slot-touching test in the crate and start
    /// from an empty slot.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lock_active().take();
        guard
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    mod live {
        use std::net::SocketAddr;
        use std::task::{Wake, Waker};

        use super::*;
        use crate::{AuthSession, AuthSessionRequest};

        const AUTHORIZATION_URL: &str = "https://as.example/authorize?client_id=app";
        const QUERY: &str = "code=a%2Bb&state=xyz&iss=https%3A%2F%2Fas.example";

        /// Wakes the test thread parked in [`block_on`].
        struct ThreadWaker(thread::Thread);

        impl Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }

        /// Drive `fut` to completion on this thread, failing the test after
        /// 15 s rather than hanging it.
        fn block_on<F: Future + Unpin>(fut: &mut F) -> F::Output {
            let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
            let mut cx = Context::from_waker(&waker);
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let Poll::Ready(value) = Pin::new(&mut *fut).poll(&mut cx) {
                    return value;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(!remaining.is_zero(), "future did not resolve within 15 s");
                thread::park_timeout(remaining);
            }
        }

        fn poll_once<F: Future>(fut: Pin<&mut F>) -> Poll<F::Output> {
            fut.poll(&mut Context::from_waker(Waker::noop()))
        }

        fn bind(path: &str) -> LoopbackSession {
            LoopbackSession::bind(LoopbackOptions::new(path)).expect("bind")
        }

        fn start(session: LoopbackSession) -> LoopbackFuture {
            session.start_with_opener(AUTHORIZATION_URL, |_| Ok(()))
        }

        /// Send `raw` on a fresh connection and read the whole response.
        /// Write/read errors end the exchange rather than failing it — a
        /// rejected client may see its connection reset.
        fn exchange(port: u16, raw: &[u8]) -> String {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("read timeout");
            let _ = stream.write_all(raw);
            read_all(&mut stream)
        }

        fn read_all(stream: &mut TcpStream) -> String {
            let mut out = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => out.extend_from_slice(&buf[..read]),
                }
            }
            String::from_utf8_lossy(&out).into_owned()
        }

        fn get(port: u16, target: &str) -> String {
            exchange(
                port,
                format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes(),
            )
        }

        fn callback_url(port: u16) -> String {
            format!("http://127.0.0.1:{port}/callback?{QUERY}")
        }

        /// Send the good callback request and assert the future resolves to
        /// it.
        fn succeed(port: u16, fut: &mut LoopbackFuture) {
            let response = get(port, &format!("/callback?{QUERY}"));
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response:?}");
            assert_eq!(
                block_on(fut),
                Ok(AuthSessionOutcome::Callback(callback_url(port)))
            );
        }

        /// Assert `port` refuses a connection attempt started within 2 s.
        /// A closed port answers `ConnectionRefused` or `ConnectionReset`
        /// depending on the OS; a still-open one is retried every 25 ms.
        fn assert_port_closes(port: u16) {
            assert_port_closes_within(port, Duration::from_secs(2));
        }

        /// [`assert_port_closes`], parameterized on how soon the port must
        /// close — the interruptible-I/O tests need a tight bound (the
        /// cancel/drop/timeout cases should close well under their
        /// mid-connection head budget, not merely by the time it expires).
        fn assert_port_closes_within(port: u16, within: Duration) {
            let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            let deadline = Instant::now() + within;
            loop {
                match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset
                        ) =>
                    {
                        return;
                    }
                    _ => {}
                }
                assert!(
                    Instant::now() < deadline,
                    "port {port} still accepts connections after {within:?}"
                );
                thread::sleep(POLL_INTERVAL);
            }
        }

        fn assert_slot_free() {
            drop(LoopbackSession::bind(LoopbackOptions::new("/callback")).expect("slot free"));
        }

        /// [`head_budget`]'s test-only seam: shrinks the per-connection head
        /// budget for the lifetime of the guard, resetting it to the real
        /// [`CONNECTION_IO_TIMEOUT`] on drop. Used only by the flood test
        /// below, under `serial()`'s exclusive lock, so no other test ever
        /// observes the shrunk value.
        struct HeadBudgetOverride;

        impl HeadBudgetOverride {
            fn millis(millis: u64) -> Self {
                TEST_HEAD_BUDGET_MS.store(millis, Ordering::SeqCst);
                Self
            }
        }

        impl Drop for HeadBudgetOverride {
            fn drop(&mut self) {
                TEST_HEAD_BUDGET_MS.store(0, Ordering::SeqCst);
            }
        }

        #[test]
        fn bind_yields_a_127_0_0_1_redirect_uri() {
            let _guard = serial();
            assert!(LoopbackSession::is_supported());
            let session = bind("/oauth/callback");
            let addr = session.listener.local_addr().expect("local addr");
            assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
            assert_ne!(addr.port(), 0);
            assert_eq!(session.port, addr.port());
            assert_eq!(
                session.redirect_uri(),
                format!("http://127.0.0.1:{}/oauth/callback", addr.port())
            );
        }

        #[test]
        fn wrong_path_is_404_and_the_session_keeps_waiting() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut fut = start(session);
            for target in [
                "/other",
                "/callbackx",
                "/callback/",
                "/",
                "/Callback?code=1",
            ] {
                let response = get(port, target);
                assert!(
                    response.starts_with("HTTP/1.1 404 Not Found\r\n"),
                    "{target}"
                );
                assert!(response.ends_with(NOT_FOUND_BODY));
            }
            assert_eq!(poll_once(Pin::new(&mut fut)), Poll::Pending);
            succeed(port, &mut fut);
        }

        #[test]
        fn post_and_head_are_404_and_the_session_keeps_waiting() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut fut = start(session);
            for method in ["POST", "HEAD", "get"] {
                let response = exchange(
                    port,
                    format!(
                        "{method} /callback?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                         Content-Length: 0\r\n\r\n"
                    )
                    .as_bytes(),
                );
                assert!(
                    response.starts_with("HTTP/1.1 404 Not Found\r\n"),
                    "{method}"
                );
            }
            succeed(port, &mut fut);
        }

        /// A no-cors cross-origin request — the browser's own Fetch
        /// Metadata tag for a context that can never be a top-level
        /// navigation, e.g. a port scan from any open web origin — is `404`
        /// and the session keeps waiting.
        #[test]
        fn sec_fetch_metadata_from_a_forbidden_context_is_404_and_the_session_keeps_waiting() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut fut = start(session);
            let response = exchange(
                port,
                format!(
                    "GET /callback?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                     Sec-Fetch-Mode: no-cors\r\nSec-Fetch-Dest: empty\r\n\
                     Sec-Fetch-Site: cross-site\r\n\r\n"
                )
                .as_bytes(),
            );
            assert!(
                response.starts_with("HTTP/1.1 404 Not Found\r\n"),
                "{response:?}"
            );
            assert!(response.ends_with(NOT_FOUND_BODY));
            succeed(port, &mut fut);
        }

        /// A browser's top-level navigation carries `Sec-Fetch-Mode:
        /// navigate` and `Sec-Fetch-Dest: document` — accepted exactly as a
        /// request with no Fetch Metadata headers at all.
        #[test]
        fn sec_fetch_metadata_matching_a_top_level_navigation_resolves_the_callback() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut fut = start(session);
            let response = exchange(
                port,
                format!(
                    "GET /callback?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                     Sec-Fetch-Mode: navigate\r\nSec-Fetch-Dest: document\r\n\
                     Sec-Fetch-Site: cross-site\r\nSec-Fetch-User: ?1\r\n\r\n"
                )
                .as_bytes(),
            );
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response:?}");
            assert_eq!(
                block_on(&mut fut),
                Ok(AuthSessionOutcome::Callback(callback_url(port)))
            );
        }

        /// `Sec-Fetch-Mode: cors` alone (no `Sec-Fetch-Dest`) is still `404`
        /// — one mismatching Fetch Metadata header is enough.
        #[test]
        fn sec_fetch_mode_cors_alone_is_404_and_the_session_keeps_waiting() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut fut = start(session);
            let response = exchange(
                port,
                format!(
                    "GET /callback?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                     Sec-Fetch-Mode: cors\r\n\r\n"
                )
                .as_bytes(),
            );
            assert!(
                response.starts_with("HTTP/1.1 404 Not Found\r\n"),
                "{response:?}"
            );
            succeed(port, &mut fut);
        }

        /// The opener sees exactly the authorization URL, and the completed
        /// future stays `Pending`.
        #[test]
        fn success_resolves_the_callback_with_its_query_intact() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut opened = None;
            let mut fut = session.start_with_opener(AUTHORIZATION_URL, |url| {
                opened = Some(url.to_string());
                Ok(())
            });
            assert_eq!(opened.as_deref(), Some(AUTHORIZATION_URL));

            let response = get(port, &format!("/callback?{QUERY}"));
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response:?}");
            assert!(response.contains("\r\nCache-Control: no-store\r\n"));
            assert!(response.contains("\r\nContent-Security-Policy: default-src 'none'\r\n"));
            assert!(response.contains("Sign-in complete."));
            assert!(!response.contains("<script"));
            assert!(!response.contains("xyz"));
            assert!(!response.contains("a%2Bb"));

            assert_eq!(
                block_on(&mut fut),
                Ok(AuthSessionOutcome::Callback(callback_url(port)))
            );
            assert_eq!(poll_once(Pin::new(&mut fut)), Poll::Pending);
            assert!(lock_active().is_none(), "success releases the slot");
        }

        #[test]
        fn a_second_connection_after_success_is_refused() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut fut = start(session);
            succeed(port, &mut fut);
            assert_port_closes(port);
        }

        #[test]
        fn timeout_resolves_timed_out_and_closes_the_port() {
            let _guard = serial();
            let session = LoopbackSession::bind(LoopbackOptions {
                path: "/callback".to_string(),
                timeout: Duration::from_millis(200),
            })
            .expect("bind");
            let port = session.port;
            let mut fut = start(session);
            assert_eq!(block_on(&mut fut), Err(AuthSessionError::TimedOut));
            assert_port_closes(port);
            assert_slot_free();
        }

        #[test]
        fn dropping_the_future_closes_the_port_and_frees_the_slot() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let fut = start(session);
            drop(fut);
            assert!(lock_active().is_none());
            assert_port_closes(port);
            assert_slot_free();
        }

        #[test]
        fn dropping_an_unstarted_session_closes_the_port_and_frees_the_slot() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            drop(session);
            assert!(lock_active().is_none());
            assert_port_closes(port);
            assert_slot_free();
        }

        #[test]
        fn cancel_resolves_cancelled_and_closes_the_port() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let cancel = session.cancel_handle();
            let mut fut = start(session);
            assert_eq!(poll_once(Pin::new(&mut fut)), Poll::Pending);
            let canceller = thread::spawn(move || cancel.cancel());
            canceller.join().expect("cancel thread");
            assert_eq!(block_on(&mut fut), Ok(AuthSessionOutcome::Cancelled));
            assert_port_closes(port);
            assert_slot_free();
        }

        /// A cancel before `start` resolves `Cancelled` without opening the
        /// browser.
        #[test]
        fn cancel_before_start_never_opens_the_browser() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            session.cancel_handle().cancel();
            let mut opened = false;
            let mut fut = session.start_with_opener(AUTHORIZATION_URL, |_| {
                opened = true;
                Ok(())
            });
            assert!(!opened);
            assert_eq!(block_on(&mut fut), Ok(AuthSessionOutcome::Cancelled));
            assert_port_closes(port);
        }

        #[test]
        fn a_bound_session_makes_both_backends_busy() {
            let _guard = serial();
            let request = AuthSessionRequest {
                url: "https://issuer.example/authorize".to_string(),
                callback_scheme: "frustplay".to_string(),
                ephemeral: false,
            };
            let session = bind("/callback");
            assert_eq!(
                LoopbackSession::bind(LoopbackOptions::new("/callback")).unwrap_err(),
                AuthSessionError::Busy
            );
            {
                let mut custom = std::pin::pin!(AuthSession::start(request.clone()));
                assert_eq!(
                    poll_once(custom.as_mut()),
                    Poll::Ready(Err(AuthSessionError::Busy))
                );
            }
            drop(session);

            assert_slot_free();
            let mut custom = std::pin::pin!(AuthSession::start(request));
            assert_ne!(
                poll_once(custom.as_mut()),
                Poll::Ready(Err(AuthSessionError::Busy))
            );
        }

        /// Also covers the other `400` request shapes.
        #[test]
        fn malformed_requests_are_400_and_the_session_keeps_waiting() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut fut = start(session);
            let requests = [
                format!("GET /callback?{QUERY} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"),
                format!("GET /callback?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
                format!(
                    "GET /callback?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
                    port.wrapping_add(1)
                ),
                format!("GET /callback?{QUERY} HTTP/1.1\r\n\r\n"),
                format!(
                    "GET /callback?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                     host: 127.0.0.1:{port}\r\n\r\n"
                ),
                format!("GET /callback?{QUERY} HTTP/2.0\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
                format!("GET /callback?{QUERY}\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
                format!("GET callback HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
                format!("GET /callback#x HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
                format!("GET /callback?a=<b> HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
                format!("GET /callback?a=%zz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
                format!("GET /callback HTTP/1.1\r\nHost 127.0.0.1:{port}\r\n\r\n"),
                format!("GET /callback HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n folded\r\n\r\n"),
                format!(
                    "GET /callback?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                     Sec-Fetch-Mode: navigate\r\nSec-Fetch-Mode: navigate\r\n\r\n"
                ),
                format!(
                    "GET /callback?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                     Sec-Fetch-Dest: document\r\nSec-Fetch-Dest: document\r\n\r\n"
                ),
                "garbage\r\n\r\n".to_string(),
            ];
            for request in &requests {
                let response = exchange(port, request.as_bytes());
                assert!(
                    response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
                    "{request:?} -> {response:?}"
                );
                assert!(response.ends_with(BAD_REQUEST_BODY));
                assert!(!response.contains("xyz"), "nothing is echoed");
            }
            // The Host comparison is ASCII-case-insensitive and OWS-trimmed.
            let response = exchange(
                port,
                format!("GET /callback?{QUERY} HTTP/1.0\r\nhOsT:  127.0.0.1:{port} \r\n\r\n")
                    .as_bytes(),
            );
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response:?}");
            assert_eq!(
                block_on(&mut fut),
                Ok(AuthSessionOutcome::Callback(callback_url(port)))
            );
        }

        #[test]
        fn an_oversized_head_is_400_and_the_session_keeps_waiting() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut fut = start(session);
            let request = format!(
                "GET /callback?{QUERY} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: {}\r\n\r\n",
                "a".repeat(MAX_REQUEST_HEAD + 512)
            );
            let response = exchange(port, request.as_bytes());
            assert!(
                response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
                "{response:?}"
            );
            succeed(port, &mut fut);
        }

        #[test]
        fn a_silent_client_is_cut_off_and_a_later_request_succeeds() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut fut = start(session);
            let started = Instant::now();
            let mut silent = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
            silent
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("read timeout");
            let response = read_all(&mut silent);
            let elapsed = started.elapsed();
            assert!(
                response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
                "{response:?}"
            );
            assert!(
                elapsed < Duration::from_millis(3500),
                "cut off after {elapsed:?}"
            );
            drop(silent);
            succeed(port, &mut fut);
        }

        /// A cancel fired while a silent client is connected still resolves
        /// `Cancelled` and closes the port promptly — within about one poll
        /// interval of the cancel, not the connection's 2 s head budget —
        /// because the accept loop's read of that connection is itself
        /// interruptible.
        #[test]
        fn cancel_resolves_promptly_despite_a_silent_connection() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let cancel = session.cancel_handle();
            let mut fut = start(session);
            let silent = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
            thread::sleep(Duration::from_millis(50));

            let started = Instant::now();
            cancel.cancel();
            assert_eq!(block_on(&mut fut), Ok(AuthSessionOutcome::Cancelled));
            let elapsed = started.elapsed();
            assert!(
                elapsed < Duration::from_millis(200),
                "resolved after {elapsed:?}"
            );
            assert_port_closes_within(port, Duration::from_millis(200));
            drop(silent);
        }

        /// The session deadline fires on time despite a silent connection
        /// sitting in the accept loop: `TimedOut` resolves within the
        /// configured timeout plus about one poll interval, not the
        /// connection's own 2 s head budget.
        #[test]
        fn timeout_resolves_promptly_despite_a_silent_connection() {
            let _guard = serial();
            let session = LoopbackSession::bind(LoopbackOptions {
                path: "/callback".to_string(),
                timeout: Duration::from_millis(300),
            })
            .expect("bind");
            let port = session.port;
            let started = Instant::now();
            let mut fut = start(session);
            let silent = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
            assert_eq!(block_on(&mut fut), Err(AuthSessionError::TimedOut));
            let elapsed = started.elapsed();
            assert!(
                elapsed < Duration::from_millis(300 + 200),
                "resolved after {elapsed:?}"
            );
            assert_port_closes_within(port, Duration::from_millis(200));
            drop(silent);
        }

        /// Dropping the future while a silent client is connected closes
        /// the port promptly rather than waiting out that connection's own
        /// 2 s head budget.
        #[test]
        fn dropping_the_future_closes_the_port_promptly_despite_a_silent_connection() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let fut = start(session);
            let silent = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
            drop(fut);
            assert_port_closes_within(port, Duration::from_millis(200));
            drop(silent);
        }

        /// A flood of sequential silent clients that each hang does not
        /// starve a later well-formed request — each one is still cut off
        /// at its own head budget and the listener keeps waiting, exactly
        /// as a single silent client is in
        /// `a_silent_client_is_cut_off_and_a_later_request_succeeds` above.
        /// Uses [`HeadBudgetOverride`] to shrink that budget for this test
        /// only, so five sequential cutoffs don't add 10 s to the suite.
        #[test]
        fn five_silent_clients_do_not_starve_a_later_request() {
            let _guard = serial();
            let _budget = HeadBudgetOverride::millis(300);
            let session = bind("/callback");
            let port = session.port;
            let mut fut = start(session);
            for _ in 0..5 {
                let mut silent = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
                silent
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("read timeout");
                let response = read_all(&mut silent);
                assert!(
                    response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
                    "{response:?}"
                );
            }
            succeed(port, &mut fut);
        }

        /// The authorization-URL half of the invalid-input rule table.
        #[test]
        fn a_non_https_authorization_url_is_invalid_and_closes_the_port() {
            let _guard = serial();
            for url in ["http://as.example/authorize", "javascript:alert(1)", ""] {
                let session = bind("/callback");
                let port = session.port;
                let mut opened = false;
                let mut fut = session.start_with_opener(url, |_| {
                    opened = true;
                    Ok(())
                });
                assert!(!opened, "the browser must not open for {url:?}");
                assert_eq!(block_on(&mut fut), Err(AuthSessionError::InvalidUrl));
                assert_port_closes(port);
                drop(fut);
                assert!(lock_active().is_none());
            }
        }

        #[test]
        fn an_opener_failure_resolves_its_error_and_closes_the_port() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let mut fut =
                session.start_with_opener(AUTHORIZATION_URL, |_| Err(AuthSessionError::NoHandler));
            assert_eq!(block_on(&mut fut), Err(AuthSessionError::NoHandler));
            assert_port_closes(port);
            assert_slot_free();
        }

        #[test]
        fn debug_output_never_contains_the_query() {
            let _guard = serial();
            let session = bind("/callback");
            let port = session.port;
            let rendered = format!("{session:?}");
            assert!(rendered.contains(&port.to_string()), "{rendered}");
            assert!(rendered.contains("/callback"), "{rendered}");
            assert!(!format!("{:?}", session.cancel_handle()).contains("127.0.0.1"));

            let mut fut = start(session);
            let response = get(port, &format!("/callback?{QUERY}"));
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
            let outcome = block_on(&mut fut);
            for rendered in [format!("{outcome:?}"), format!("{:?}", outcome.clone())] {
                assert!(!rendered.contains("xyz"), "{rendered}");
                assert!(!rendered.contains("code="), "{rendered}");
            }
            let timed_out = AuthSessionError::TimedOut;
            assert!(!timed_out.to_string().contains("http"));
            assert!(!format!("{timed_out:?}").contains("http"));
        }

        #[test]
        fn launcher_errors_map_variant_for_variant() {
            use frust_url_launcher::UrlLauncherError;
            assert_eq!(
                map_launcher_error(UrlLauncherError::InvalidUrl),
                AuthSessionError::InvalidUrl
            );
            assert_eq!(
                map_launcher_error(UrlLauncherError::NoHandler),
                AuthSessionError::NoHandler
            );
            assert_eq!(
                map_launcher_error(UrlLauncherError::PlatformNotInitialized),
                AuthSessionError::PlatformNotInitialized
            );
            assert_eq!(
                map_launcher_error(UrlLauncherError::Platform("spawn failed".to_string())),
                AuthSessionError::Platform("spawn failed".to_string())
            );
        }
    }
}
