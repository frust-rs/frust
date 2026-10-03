//! The static development server a browser build is previewed through: a
//! `std`-only HTTP/1.1 file server over the artifact directory
//! [`super::build`] produced.
//!
//! # Why this exists at all, rather than "use any static server"
//!
//! One header. A browser refuses to *streaming*-compile a WebAssembly module
//! served with anything other than `application/wasm`
//! (`WebAssembly.instantiateStreaming`'s own MIME check), and the artifact
//! directory's whole payload is one multi-megabyte `.wasm`. `wasm-bindgen`'s
//! generated glue does carry an `arrayBuffer()` fallback for a server that
//! answers wrongly (`crates/frust-shell-web/platform/web/README.md` records this), so a bad
//! `Content-Type` degrades rather than breaks — but it degrades by buffering
//! the entire module before compiling any of it, which is the difference
//! between a preview that starts while it downloads and one that stares at a
//! blank page first. Serving the right type is the single reason a drive-owned
//! server is worth its own module; [`CONTENT_TYPES`] is where that decision
//! lives, and it is a table, not a lookup into the host's MIME database
//! (`/etc/mime.types` varies per machine and has answered `.wasm` wrongly on
//! plenty of them).
//!
//! Second reason, same size: **no caching**. A preview loop rebuilds the same
//! two file names over and over — a cached `app_bg.wasm` is a rebuild the
//! developer cannot see. Every response carries the full no-store set, so a
//! reload always fetches what the last build actually wrote.
//!
//! # No dependency
//!
//! `frust-drive`'s dependency set is a charter boundary
//! (`docs/CLI_ARCHITECTURE.md` § Layer Dependencies) and a dev server is not
//! worth widening it: this is `std::net::TcpListener` plus a request parser
//! small enough to read in one sitting. It speaks the subset of HTTP/1.1 a
//! browser needs to load a page — `GET`/`HEAD`, `Connection: close`,
//! `Content-Length`-delimited responses — and nothing else. No keep-alive, no
//! chunked encoding, no range requests, no compression, no TLS, no directory
//! listing. It binds loopback by default and is not a production server; that
//! is a documented limit of this module, not an omission to be fixed by
//! degrees.
//!
//! # Thread-per-connection
//!
//! The accept loop runs on one background thread and hands each accepted
//! socket to its own short-lived thread. A single-threaded server would be
//! simpler still, but a browser opens several parallel connections per page
//! and holds some of them idle: with sequential handling, one idle preconnect
//! stalls every queued request behind it until the read timeout fires. Each
//! connection carries its own read/write timeouts so a stalled peer costs one
//! thread for a bounded time rather than the server.

use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::manifest::Manifest;

/// The port [`ServeOptions::default`] binds, and the port
/// [`ServeOptions::from_manifest`] falls back to for a project with **no**
/// `frust.toml` at all (`examples/web-gallery`'s shape). A project that does
/// carry a manifest gets `[web] port` instead (`WebSection::port_or_default`,
/// default `8000`) — see [`ServeOptions::from_manifest`].
///
/// Deliberately **not** 8080: that port is the one a developer most often
/// already has something on (and this repo's own guardrails refuse it), and a
/// preview that fails to bind is a worse first experience than an unfamiliar
/// number. 8930 sits immediately below the 8931-8934 block the web tier's own
/// verification recipes use (`examples/web-gallery/README.md`,
/// `examples/web-spike/README.md`), so the default can never collide with a
/// documented manual-proof server run alongside it.
pub const DEFAULT_PORT: u16 = 8930;

/// How long an accepted connection may go without sending a complete request
/// before it is dropped. Bounds the cost of a browser preconnect that never
/// sends anything.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a response write may stall. Generous because the payload is a
/// multi-megabyte `.wasm`, though on loopback it never approaches this.
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

/// Bound on how long [`handle_connection`] waits, after it has written its
/// response, to drain any bytes the peer might still have in flight before
/// the connection is dropped.
///
/// This exists for one platform-specific reason: closing a socket while
/// unread bytes remain in its receive buffer makes some TCP stacks —
/// Windows chief among them — answer with an abortive RST instead of a
/// graceful FIN, which the peer sees as a forced reset
/// (`ConnectionReset`, Windows `WSAECONNRESET` / os error 10054) rather
/// than a clean end-of-stream, even though the response it wanted had
/// already been delivered in full. This server only ever reads up through
/// a request's blank line (see `read_request`) and never reads a body —
/// so a request this server refuses with a body attached (a `POST`, say)
/// or a peer whose write races the response leaves exactly that unread
/// tail behind. Draining it here, bounded and best-effort, keeps the
/// close graceful for a well-behaved client without giving a slow or
/// silent peer any way to hold a connection thread open. Short because
/// this server's `Connection: close` contract means a well-behaved peer
/// has nothing left to send once it has read the response.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(200);

/// How often the accept loop wakes to notice [`DevServer::shutdown`]. Polling
/// a non-blocking listener costs one wake-up per interval and needs no
/// self-connect trick to unblock a parked `accept`.
const ACCEPT_POLL: Duration = Duration::from_millis(15);

/// Upper bound on a request head (request line plus headers). A dev server has
/// no reason to accept more, and the cap is what keeps a hostile or broken
/// peer from growing a buffer without limit.
const MAX_REQUEST_HEAD: usize = 8 * 1024;

/// The response headers every reply carries, no-cache first: a preview loop
/// rewrites the same file names on every rebuild, so a cached response is a
/// rebuild the developer cannot see. All three of the historical no-cache
/// spellings are sent because the set has to hold for every browser a preview
/// might be opened in, not only the newest.
const NO_CACHE_HEADERS: &str = "Cache-Control: no-store, no-cache, must-revalidate, max-age=0\r\n\
     Pragma: no-cache\r\n\
     Expires: 0\r\n";

/// Extension → `Content-Type`, lowercase extensions, longest-lived decision in
/// this module.
///
/// `wasm` is the load-bearing row (see the module doc). The rest cover what a
/// `wasm-bindgen --target web` output directory plus `crates/frust-shell-web/platform/web`'s host
/// page actually contain, plus the ordinary static assets an app author drops
/// beside them. Anything unlisted falls back to
/// [`DEFAULT_CONTENT_TYPE`] rather than being guessed at: a wrong specific
/// type can make a browser misinterpret a file, while
/// `application/octet-stream` only makes it decline to.
///
/// `ts` maps to plain text, not `video/mp2t` (the IANA registration for a
/// transport stream): the only `.ts` files this server ever sees are
/// `wasm-bindgen`'s `.d.ts` type declarations, which no browser ever requests
/// — a type that renders as text is the useful answer for a human who opens
/// one, and the transport-stream reading is meaningless in an artifact
/// directory.
pub const CONTENT_TYPES: &[(&str, &str)] = &[
    ("wasm", "application/wasm"),
    ("js", "text/javascript; charset=utf-8"),
    ("mjs", "text/javascript; charset=utf-8"),
    ("html", "text/html; charset=utf-8"),
    ("htm", "text/html; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("json", "application/json"),
    ("map", "application/json"),
    ("ts", "text/plain; charset=utf-8"),
    ("txt", "text/plain; charset=utf-8"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("ico", "image/x-icon"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
    ("wav", "audio/wav"),
    ("mp3", "audio/mpeg"),
    ("mp4", "video/mp4"),
];

/// What an extension outside [`CONTENT_TYPES`] is served as.
pub const DEFAULT_CONTENT_TYPE: &str = "application/octet-stream";

/// The `Content-Type` this server answers a request for `path` with.
///
/// Case-insensitive on the extension (`APP.WASM` is served as
/// `application/wasm`), and extension-only: nothing is sniffed from the file's
/// contents.
pub fn content_type(path: &Path) -> &'static str {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return DEFAULT_CONTENT_TYPE;
    };
    let ext = ext.to_ascii_lowercase();
    CONTENT_TYPES
        .iter()
        .find(|(name, _)| *name == ext)
        .map(|(_, mime)| *mime)
        .unwrap_or(DEFAULT_CONTENT_TYPE)
}

/// Where and how [`serve`] binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServeOptions {
    /// The interface to bind. Loopback by default — a development build is
    /// unsigned, unminified and served without TLS, so exposing it on a LAN is
    /// an explicit choice a front-end has to make, never this module's
    /// default.
    pub host: IpAddr,
    /// The port to bind. `0` asks the OS for an ephemeral one, readable
    /// afterwards through [`DevServer::local_addr`] — how a test binds without
    /// racing another for a fixed number.
    pub port: u16,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: DEFAULT_PORT,
        }
    }
}

impl ServeOptions {
    /// Loopback, on the port `manifest`'s `[web]` section names —
    /// `WebSection::port_or_default` (default `8000`) when `manifest` is
    /// `Some`, else [`DEFAULT_PORT`] (`8930`) for a project with no
    /// `frust.toml` at all. `manifest` is `Option` rather than a bare
    /// reference for exactly that reason: a manifest-less project
    /// (`examples/web-gallery`'s shape) is a valid input this pipeline keeps
    /// working for (see `super::super`'s module doc), and it has no `[web]
    /// port` to read.
    ///
    /// A present manifest with no `[web]` section still resolves to `8000`,
    /// not `8930` — `WebSection::default().port_or_default()` — so 8930 is
    /// reserved for the no-manifest-at-all case specifically, never for "a
    /// manifest exists but says nothing about the port".
    pub fn from_manifest(manifest: Option<&Manifest>) -> Self {
        let port = match manifest {
            Some(manifest) => manifest.web.clone().unwrap_or_default().port_or_default(),
            None => DEFAULT_PORT,
        };
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
        }
    }
}

/// A sink for one-line request traces (`GET /pkg/app.js -> 200`), shared
/// across the connection threads.
///
/// `frust-drive` cores are print-free (`docs/CODE_STANDARDS.md`'s printing
/// anti-pattern), so the server never writes to stdout itself; a front-end
/// that wants a request log passes one of these and renders it its own way.
/// `Send + Sync` because every connection thread calls it.
pub type RequestLog = Arc<dyn Fn(&str) + Send + Sync>;

/// Everything that can stop a dev server from starting.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error(
        "'{path}' is not a directory to serve — build the app first \
         (`frust build web`) so the artifact directory exists"
    )]
    RootMissing { path: PathBuf },
    #[error(
        "binding {addr}: {source} — another process may already be using that \
         port; pass a different one, or 0 to let the OS pick"
    )]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
}

/// A running dev server. Serving stops when this is dropped or
/// [`shutdown`](DevServer::shutdown) is called — there is no way to hold a
/// bound port without holding this value, so a front-end cannot leak one by
/// forgetting to stop it.
pub struct DevServer {
    addr: SocketAddr,
    root: PathBuf,
    shutdown: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl fmt::Debug for DevServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevServer")
            .field("addr", &self.addr)
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl DevServer {
    /// The address actually bound — the one to print, and the only way to
    /// learn the port when [`ServeOptions::port`] was `0`.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The directory being served.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The URL to open a browser at. Built from [`local_addr`](Self::local_addr),
    /// so it always carries the resolved port.
    pub fn url(&self) -> String {
        match self.addr.ip() {
            IpAddr::V4(ip) => format!("http://{ip}:{}/", self.addr.port()),
            IpAddr::V6(ip) => format!("http://[{ip}]:{}/", self.addr.port()),
        }
    }

    /// Stops accepting and waits for the accept loop to finish. In-flight
    /// connection threads are detached and finish on their own — each is
    /// bounded by [`WRITE_TIMEOUT`], and a preview server has nothing to
    /// flush.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.accept.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for DevServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Serves `root` over HTTP on `options`, returning as soon as the port is
/// bound — the listener is live before this returns, so a caller may open a
/// browser at [`DevServer::url`] immediately with no race.
///
/// `on_request` receives one line per handled request; pass `None` for a
/// silent server.
pub fn serve(
    root: &Path,
    options: ServeOptions,
    on_request: Option<RequestLog>,
) -> Result<DevServer, ServeError> {
    if !root.is_dir() {
        return Err(ServeError::RootMissing {
            path: root.to_path_buf(),
        });
    }
    let addr = SocketAddr::new(options.host, options.port);
    let listener = TcpListener::bind(addr).map_err(|source| ServeError::Bind { addr, source })?;
    let bound = listener
        .local_addr()
        .map_err(|source| ServeError::Bind { addr, source })?;
    listener
        .set_nonblocking(true)
        .map_err(|source| ServeError::Bind {
            addr: bound,
            source,
        })?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let root = root.to_path_buf();
    let accept = {
        let shutdown = Arc::clone(&shutdown);
        let root = root.clone();
        thread::Builder::new()
            .name("frust-web-serve".to_string())
            .spawn(move || accept_loop(listener, root, shutdown, on_request))
            .map_err(|source| ServeError::Bind {
                addr: bound,
                source,
            })?
    };

    Ok(DevServer {
        addr: bound,
        root,
        shutdown,
        accept: Some(accept),
    })
}

fn accept_loop(
    listener: TcpListener,
    root: PathBuf,
    shutdown: Arc<AtomicBool>,
    on_request: Option<RequestLog>,
) {
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let root = root.clone();
                let log = on_request.clone();
                // Detached: a connection outlives neither the process nor its
                // own timeouts, and joining it would serialize the server back
                // into the single-threaded shape the module doc rejects.
                let spawned = thread::Builder::new()
                    .name("frust-web-conn".to_string())
                    .spawn(move || handle_connection(stream, &root, log.as_deref()));
                if spawned.is_err() {
                    // Out of threads: drop the connection rather than take the
                    // process down. The browser retries.
                    continue;
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => thread::sleep(ACCEPT_POLL),
            // Any other accept error is per-connection (a peer that vanished
            // between the SYN and the accept), never a reason to stop serving.
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
}

/// One request, one response, then close — the `Connection: close` contract
/// this server advertises on every reply.
fn handle_connection(stream: TcpStream, root: &Path, log: Option<&(dyn Fn(&str) + Send + Sync)>) {
    // The listener is non-blocking (see `serve`, so `accept_loop` can poll the
    // shutdown flag), and on macOS/BSD an accepted socket INHERITS its
    // listener's O_NONBLOCK (unlike Linux, where `accept` always returns a
    // blocking socket). Left non-blocking here, the read timeout set below
    // would do nothing: the first `read_line` in `read_request` returns
    // `WouldBlock` immediately whenever the request bytes have not arrived
    // yet, and that reads as a dead connection to close rather than a slow
    // one to wait on. Clear it before the timeouts are set so they actually
    // apply; a stream that refuses is dropped the same way a failed
    // `try_clone` below is.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    });
    let mut writer = stream;

    let request = match read_request(&mut reader) {
        Ok(Some(request)) => request,
        // A malformed head still gets an answer: a browser reports a refused
        // connection as a network error with no detail, while a 400 shows up.
        Ok(None) => {
            let _ = respond(
                &mut writer,
                Status::BadRequest,
                b"malformed request",
                None,
                false,
            );
            finish_connection(&writer, &mut reader);
            return;
        }
        Err(_) => return,
    };

    let head_only = request.method == "HEAD";
    let status = if request.method != "GET" && !head_only {
        let _ = respond(
            &mut writer,
            Status::MethodNotAllowed,
            b"only GET and HEAD are served",
            None,
            false,
        );
        Status::MethodNotAllowed
    } else if let Some(host) = &request.host {
        // Validate Host header for DNS-rebinding defence.
        if !validate_host_header(host) {
            let _ = respond(
                &mut writer,
                Status::BadRequest,
                b"invalid Host header",
                None,
                false,
            );
            Status::BadRequest
        } else {
            match resolve_request_path(root, &request.target) {
                Ok(path) => {
                    let mime = content_type(&path);
                    match fs::read(&path) {
                        Ok(body) => {
                            let _ = respond(&mut writer, Status::Ok, &body, Some(mime), head_only);
                            Status::Ok
                        }
                        // Readable a moment ago, unreadable now (a rebuild
                        // replacing the file mid-request is the realistic case).
                        Err(_) => {
                            let _ = respond(
                                &mut writer,
                                Status::ServerError,
                                b"could not read the requested file",
                                None,
                                head_only,
                            );
                            Status::ServerError
                        }
                    }
                }
                Err(status) => {
                    let _ = respond(
                        &mut writer,
                        status,
                        status.reason().as_bytes(),
                        None,
                        head_only,
                    );
                    status
                }
            }
        }
    } else {
        // No Host header provided; accept for now (browsers always send it).
        match resolve_request_path(root, &request.target) {
            Ok(path) => {
                let mime = content_type(&path);
                match fs::read(&path) {
                    Ok(body) => {
                        let _ = respond(&mut writer, Status::Ok, &body, Some(mime), head_only);
                        Status::Ok
                    }
                    // Readable a moment ago, unreadable now (a rebuild
                    // replacing the file mid-request is the realistic case).
                    Err(_) => {
                        let _ = respond(
                            &mut writer,
                            Status::ServerError,
                            b"could not read the requested file",
                            None,
                            head_only,
                        );
                        Status::ServerError
                    }
                }
            }
            Err(status) => {
                let _ = respond(
                    &mut writer,
                    status,
                    status.reason().as_bytes(),
                    None,
                    head_only,
                );
                status
            }
        }
    };

    if let Some(log) = log {
        log(&format!(
            "{} {} -> {}",
            sanitize_log_field(&request.method),
            sanitize_log_field(&request.target),
            status.code()
        ));
    }

    finish_connection(&writer, &mut reader);
}

/// Half-closes the write side — sending the FIN this server's
/// `Connection: close` contract promises — then drains, bounded by
/// [`DRAIN_TIMEOUT`], any bytes the peer might still be sending before the
/// connection is dropped. See [`DRAIN_TIMEOUT`] for why: this server never
/// reads a request body (`read_request` stops at the blank line), so a
/// refused request that carried one (a `POST`, say) leaves it sitting
/// unread in the socket's receive buffer, and closing with unread bytes
/// still there is exactly the condition that makes a platform answer with
/// an abortive reset instead of a graceful close.
fn finish_connection(writer: &TcpStream, reader: &mut BufReader<TcpStream>) {
    let _ = writer.shutdown(Shutdown::Write);
    let _ = reader.get_ref().set_read_timeout(Some(DRAIN_TIMEOUT));
    let mut discard = [0u8; 1024];
    loop {
        match reader.read(&mut discard) {
            Ok(0) | Err(_) => break,
            Ok(_) => continue,
        }
    }
}

/// Longest request-line field the request log reproduces; anything past it
/// is replaced by an ellipsis. A target is attacker-supplied text headed for
/// a terminal, so it is bounded as well as scrubbed.
const MAX_LOGGED_FIELD: usize = 512;

/// Makes an attacker-supplied request-line field safe to print: every ASCII
/// control character (`\x00`–`\x1f` and DEL — the bytes that carry terminal
/// escape sequences, the CR/LF that would forge a second log line, and NUL)
/// becomes a space, printable text passes through untouched, and a field
/// longer than [`MAX_LOGGED_FIELD`] characters is cut with an ellipsis.
fn sanitize_log_field(field: &str) -> String {
    let mut out: String = field
        .chars()
        .take(MAX_LOGGED_FIELD)
        .map(|c| if c.is_ascii_control() { ' ' } else { c })
        .collect();
    if field.chars().count() > MAX_LOGGED_FIELD {
        out.push('\u{2026}');
    }
    out
}

/// The request line's method and target, plus the Host header.
/// Other headers are consumed to find the end of the head and then discarded.
struct Request {
    method: String,
    target: String,
    host: Option<String>,
}

/// Reads the request head, returning `Ok(None)` for a head that is malformed
/// or over [`MAX_REQUEST_HEAD`], and `Err` only when the socket itself failed
/// (a timeout, a reset) and there is nobody left to answer.
fn read_request(reader: &mut BufReader<TcpStream>) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    let mut read = reader
        .by_ref()
        .take(MAX_REQUEST_HEAD as u64)
        .read_line(&mut line)?;
    if read == 0 {
        // Peer connected and closed without sending anything (a browser
        // preconnect). Nothing to answer.
        return Err(std::io::Error::from(ErrorKind::UnexpectedEof));
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Ok(None);
    };
    let mut request = Request {
        method: method.to_string(),
        target: target.to_string(),
        host: None,
    };

    // Read headers up to the blank line terminating the head, bounded by the
    // same cap. Extract the Host header; a body (a POST this server refuses)
    // is never read: the response closes the connection regardless.
    let mut consumed = read;
    loop {
        if consumed >= MAX_REQUEST_HEAD {
            return Ok(None);
        }
        let mut header = String::new();
        read = reader
            .by_ref()
            .take((MAX_REQUEST_HEAD - consumed) as u64)
            .read_line(&mut header)?;
        consumed += read;
        if read == 0 || header.trim_end().is_empty() {
            break;
        }
        // Extract Host header if present.
        if header.to_lowercase().starts_with("host:") {
            request.host = Some(header[5..].trim().to_string());
        }
    }
    Ok(Some(request))
}

/// The response statuses this server produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    BadRequest,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    ServerError,
}

impl Status {
    pub fn code(self) -> u16 {
        match self {
            Status::Ok => 200,
            Status::BadRequest => 400,
            Status::Forbidden => 403,
            Status::NotFound => 404,
            Status::MethodNotAllowed => 405,
            Status::ServerError => 500,
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::BadRequest => "Bad Request",
            Status::Forbidden => "Forbidden",
            Status::NotFound => "Not Found",
            Status::MethodNotAllowed => "Method Not Allowed",
            Status::ServerError => "Internal Server Error",
        }
    }
}

/// Validates that the Host header contains only loopback addresses.
/// Returns `true` if the host is valid (localhost, 127.0.0.1, or [::1]),
/// or `false` otherwise (DNS-rebinding defence).
fn validate_host_header(host: &str) -> bool {
    let host = host.trim();

    if host.starts_with('[') {
        // IPv6 format: [::1] or [::1]:port
        let host_part = if let Some(bracket_end) = host.find(']') {
            &host[1..bracket_end]
        } else {
            return false;
        };
        return host_part == "::1";
    }

    // IPv4 or hostname: may have optional :port
    let host_part = host.split(':').next().unwrap_or(host);

    host_part == "localhost" || host_part == "127.0.0.1"
}

fn respond(
    writer: &mut TcpStream,
    status: Status,
    body: &[u8],
    mime: Option<&str>,
    head_only: bool,
) -> std::io::Result<()> {
    let mime = mime.unwrap_or("text/plain; charset=utf-8");
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\n{NO_CACHE_HEADERS}",
        status.code(),
        status.reason(),
        body.len(),
    );
    if status == Status::MethodNotAllowed {
        head.push_str("Allow: GET, HEAD\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    writer.write_all(head.as_bytes())?;
    if !head_only {
        writer.write_all(body)?;
    }
    writer.flush()
}

/// Maps a request target onto a file inside `root`, or the status to answer
/// with instead.
///
/// Containment is the whole job. The target is split on `/` and rebuilt
/// component by component: a segment is accepted only if
/// `Path::new(segment).components()` yields exactly one `Component::Normal`
/// and the segment contains no `:` (defeating Windows drive-prefix bypasses).
/// After pushing all segments, containment is post-checked with
/// `resolved.starts_with(root)` as belt-and-braces. That is what keeps
/// `GET /../../etc/passwd`, its encoded spellings, and `/C:/secrets.rs`
/// from reaching a byte outside the artifact directory. Nothing is
/// canonicalized: a symlink inside the artifact directory is the
/// developer's own, and resolving links would only move the decision to a
/// place where the component check no longer applies.
///
/// A directory resolves to its `index.html`, so `/` serves the embedder's host
/// page. There is no directory listing — a preview server has nothing to list
/// that the developer did not just build.
fn resolve_request_path(root: &Path, target: &str) -> Result<PathBuf, Status> {
    // Strip the query and fragment: `index.html?module=./pkg/app.js` is the
    // host page's own documented shape, and the server serves the file.
    let path = target
        .split(['?', '#'])
        .next()
        .filter(|p| p.starts_with('/'))
        .ok_or(Status::BadRequest)?;
    let decoded = percent_decode(path).ok_or(Status::BadRequest)?;

    let mut resolved = root.to_path_buf();
    for segment in decoded.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        // Reject segments with colons (Windows drive prefixes like C:).
        if segment.contains(':') {
            return Err(Status::Forbidden);
        }
        // Validate that the segment is exactly one Normal component.
        // This rejects .., /, \, NUL, and any other special path component.
        let components: Vec<_> = Path::new(segment).components().collect();
        if components.len() != 1 {
            return Err(Status::Forbidden);
        }
        if !matches!(components[0], Component::Normal(_)) {
            return Err(Status::Forbidden);
        }
        resolved.push(segment);
    }

    // Belt-and-braces: verify that the final resolved path is still inside root.
    if !resolved.starts_with(root) {
        return Err(Status::Forbidden);
    }

    if resolved.is_dir() {
        resolved.push("index.html");
    }
    if resolved.is_file() {
        Ok(resolved)
    } else {
        Err(Status::NotFound)
    }
}

/// Decodes `%XX` escapes. `None` for a malformed escape — a truncated or
/// non-hex one is a request this server declines rather than guesses at.
///
/// Byte-oriented and then UTF-8 validated, so a multi-byte character spelled
/// as several escapes (`%C3%A9`) decodes to one character rather than two
/// replacement bytes.
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-web-serve-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An artifact directory in the shape [`super::super::build`] produces.
    fn artifact_root(tag: &str) -> PathBuf {
        let dir = temp_dir(tag);
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
        fs::write(dir.join("frust_web.js"), "export const x = 1;").unwrap();
        fs::write(dir.join("pkg/app.js"), "export default init;").unwrap();
        fs::write(dir.join("pkg/app_bg.wasm"), b"\0asm\x01\0\0\0").unwrap();
        dir
    }

    /// [`ServeOptions::from_manifest`]'s three precedence cases: no
    /// `frust.toml` at all, one present with no `[web]` section, and one
    /// with an explicit `[web] port`.
    #[test]
    fn from_manifest_follows_the_manifest_port_precedence() {
        assert_eq!(ServeOptions::from_manifest(None).port, DEFAULT_PORT);

        let bare = crate::manifest::parse("[app]\nname = \"a\"\norg = \"o\"\n").unwrap();
        assert_eq!(ServeOptions::from_manifest(Some(&bare)).port, 8000);

        let with_port =
            crate::manifest::parse("[app]\nname = \"a\"\norg = \"o\"\n\n[web]\nport = 9000\n")
                .unwrap();
        assert_eq!(ServeOptions::from_manifest(Some(&with_port)).port, 9000);

        assert_eq!(
            ServeOptions::from_manifest(None).host,
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
    }

    /// The row the whole module exists for.
    #[test]
    fn wasm_is_served_as_application_wasm() {
        assert_eq!(
            content_type(Path::new("pkg/app_bg.wasm")),
            "application/wasm"
        );
        assert_eq!(
            content_type(Path::new("PKG/APP_BG.WASM")),
            "application/wasm"
        );
    }

    #[test]
    fn the_mime_table_covers_every_file_an_artifact_dir_holds() {
        for (path, expected) in [
            ("index.html", "text/html; charset=utf-8"),
            ("frust_web.js", "text/javascript; charset=utf-8"),
            ("pkg/app.js", "text/javascript; charset=utf-8"),
            ("pkg/app.d.ts", "text/plain; charset=utf-8"),
            ("pkg/app_bg.wasm", "application/wasm"),
            ("app.css", "text/css; charset=utf-8"),
            ("data.json", "application/json"),
            ("logo.svg", "image/svg+xml"),
            ("logo.png", "image/png"),
            ("font.woff2", "font/woff2"),
        ] {
            assert_eq!(content_type(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn an_unknown_or_missing_extension_falls_back_to_octet_stream() {
        assert_eq!(content_type(Path::new("thing.zzz")), DEFAULT_CONTENT_TYPE);
        assert_eq!(content_type(Path::new("LICENSE")), DEFAULT_CONTENT_TYPE);
    }

    /// No row may map two extensions to a contradiction, and `wasm` must be
    /// present — a table this small is worth locking against a careless edit.
    #[test]
    fn the_mime_table_has_no_duplicate_extensions() {
        let mut seen: Vec<&str> = CONTENT_TYPES.iter().map(|(ext, _)| *ext).collect();
        seen.sort_unstable();
        let count = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), count, "duplicate extension in CONTENT_TYPES");
        assert!(CONTENT_TYPES.iter().any(|(ext, _)| *ext == "wasm"));
    }

    #[test]
    fn the_root_resolves_to_the_embedder_host_page() {
        let root = artifact_root("resolve-root");
        assert_eq!(
            resolve_request_path(&root, "/").unwrap(),
            root.join("index.html")
        );
        // The host page's own documented shape: a query selecting the module.
        assert_eq!(
            resolve_request_path(&root, "/index.html?module=./pkg/app.js").unwrap(),
            root.join("index.html")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_nested_file_resolves_and_a_missing_one_is_not_found() {
        let root = artifact_root("resolve-nested");
        assert_eq!(
            resolve_request_path(&root, "/pkg/app_bg.wasm").unwrap(),
            root.join("pkg/app_bg.wasm")
        );
        assert_eq!(
            resolve_request_path(&root, "/pkg/missing.js"),
            Err(Status::NotFound)
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// Traversal, in every spelling a request can carry it.
    #[test]
    fn traversal_out_of_the_artifact_dir_is_refused() {
        let root = artifact_root("resolve-traversal");
        fs::write(root.parent().unwrap().join("secret.txt"), "no").unwrap();
        for target in [
            "/../secret.txt",
            "/pkg/../../secret.txt",
            "/%2e%2e/secret.txt",
            "/..%2fsecret.txt",
            "/%2e%2e%2fsecret.txt",
        ] {
            assert_eq!(
                resolve_request_path(&root, target),
                Err(Status::Forbidden),
                "{target}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// Windows drive-prefix bypasses are rejected.
    #[test]
    fn windows_drive_prefix_is_refused() {
        let root = artifact_root("resolve-drive");
        for target in ["/C:/Windows/win.ini", "/c:/x", "/D:/etc/passwd"] {
            assert_eq!(
                resolve_request_path(&root, target),
                Err(Status::Forbidden),
                "{target}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// Percent-encoded drive prefixes are also refused.
    #[test]
    fn percent_encoded_drive_prefix_is_refused() {
        let root = artifact_root("resolve-pct-drive");
        // %43 = 'C', %3a = ':'
        for target in ["/%43%3a/Windows/win.ini", "/%63%3a/x"] {
            assert_eq!(
                resolve_request_path(&root, target),
                Err(Status::Forbidden),
                "{target}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// Segments with colons but not drive-prefix-like are also refused.
    #[test]
    fn segments_with_colons_are_refused() {
        let root = artifact_root("resolve-colon");
        for target in ["/file:with:colons.txt", "/pkg/:app.js"] {
            assert_eq!(
                resolve_request_path(&root, target),
                Err(Status::Forbidden),
                "{target}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// Segments that are only dots (not dot) are refused or produce
    /// a NotFound for missing paths. A `.` segment is skipped (empty),
    /// so `/./x` resolves to `/x` which doesn't exist (NotFound).
    /// A bare `.` component like in a path component is refused.
    #[test]
    fn dot_components_in_path_are_handled() {
        let root = artifact_root("resolve-dots");
        // `/./x` → `/x` (skipped .dot) → NotFound (file missing)
        assert_eq!(
            resolve_request_path(&root, "/./x"),
            Err(Status::NotFound),
            "/./x should skip the dot and look for /x"
        );
        // Percent-encoded dot alone: %2e / x  → same as /./x
        assert_eq!(
            resolve_request_path(&root, "/%2e/x"),
            Err(Status::NotFound),
            "/%2e/x should skip the dot and look for /x"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_target_that_is_not_a_path_or_carries_a_bad_escape_is_a_bad_request() {
        let root = artifact_root("resolve-bad");
        for target in ["index.html", "http://elsewhere/", "/%zz", "/app%2"] {
            assert_eq!(
                resolve_request_path(&root, target),
                Err(Status::BadRequest),
                "{target}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn percent_escapes_decode_including_multi_byte_characters() {
        assert_eq!(percent_decode("/pkg/%61pp.js").unwrap(), "/pkg/app.js");
        assert_eq!(percent_decode("/caf%C3%A9.txt").unwrap(), "/café.txt");
        assert!(percent_decode("/%").is_none());
    }

    /// Host header validation accepts only loopback addresses.
    #[test]
    fn host_header_validation_rejects_non_loopback() {
        assert!(!validate_host_header("evil.com"));
        assert!(!validate_host_header("192.168.1.1"));
        assert!(!validate_host_header("example.org:8000"));
        assert!(!validate_host_header("[2001:db8::1]"));
    }

    /// Host header validation accepts localhost and 127.0.0.1.
    #[test]
    fn host_header_validation_accepts_loopback() {
        assert!(validate_host_header("localhost"));
        assert!(validate_host_header("127.0.0.1"));
        assert!(validate_host_header("localhost:8000"));
        assert!(validate_host_header("127.0.0.1:8000"));
    }

    /// Host header validation accepts IPv6 ::1.
    #[test]
    fn host_header_validation_accepts_ipv6_loopback() {
        assert!(validate_host_header("[::1]"));
        assert!(validate_host_header("[::1]:8000"));
    }

    /// One raw HTTP round trip against a real bound listener — the only way to
    /// prove the header block a browser actually receives.
    fn request(addr: SocketAddr, raw: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        String::from_utf8_lossy(&response).to_string()
    }

    /// Like [`request`], but the client connects and then sleeps before
    /// writing a byte of the request — the macOS tripwire for the
    /// inherited-non-blocking bug: on this platform, `handle_connection`'s
    /// first read hit `WouldBlock` immediately whenever no bytes had arrived
    /// yet, closing the connection with no response instead of waiting out
    /// the read timeout the way `set_read_timeout` promises. The delay is far
    /// past that immediate failure and far under [`READ_TIMEOUT`]'s 5 s, so a
    /// fixed server reads it as an ordinary slow client. On Linux, where
    /// `accept` never hands out a non-blocking socket, this passes either
    /// way; it is a control against this one platform's inherited flag, not
    /// a portable timing assertion.
    fn request_after_delay(addr: SocketAddr, delay: Duration, raw: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        thread::sleep(delay);
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        String::from_utf8_lossy(&response).to_string()
    }

    fn test_server(root: &Path) -> DevServer {
        serve(
            root,
            ServeOptions {
                port: 0,
                ..ServeOptions::default()
            },
            None,
        )
        .unwrap()
    }

    #[test]
    fn a_served_wasm_response_carries_the_wasm_type_and_no_cache_headers() {
        let root = artifact_root("e2e-wasm");
        let server = test_server(&root);
        let response = request(
            server.local_addr(),
            "GET /pkg/app_bg.wasm HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(
            response.contains("Content-Type: application/wasm\r\n"),
            "{response}"
        );
        assert!(response.contains("Content-Length: 8\r\n"), "{response}");
        assert!(response.contains("Cache-Control: no-store"), "{response}");
        assert!(
            response.contains("X-Content-Type-Options: nosniff\r\n"),
            "{response}"
        );
        assert!(response.ends_with("\0asm\u{1}\0\0\0"), "{response:?}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_root_url_serves_the_host_page() {
        let root = artifact_root("e2e-index");
        let server = test_server(&root);
        assert!(server.url().starts_with("http://127.0.0.1:"));
        let response = request(
            server.local_addr(),
            "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.ends_with("<!doctype html>"), "{response}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    /// Invalid Host headers are rejected with 400.
    #[test]
    fn invalid_host_header_is_rejected() {
        let root = artifact_root("e2e-host-invalid");
        let server = test_server(&root);
        let response = request(
            server.local_addr(),
            "GET / HTTP/1.1\r\nHost: evil.com\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    /// Requests with 127.0.0.1 Host header are accepted.
    #[test]
    fn loopback_ipv4_host_header_is_accepted() {
        let root = artifact_root("e2e-host-ipv4");
        let server = test_server(&root);
        let response = request(
            server.local_addr(),
            "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    /// Requests with IPv6 loopback Host header are accepted.
    #[test]
    fn loopback_ipv6_host_header_is_accepted() {
        let root = artifact_root("e2e-host-ipv6");
        let server = test_server(&root);
        let response = request(server.local_addr(), "GET / HTTP/1.1\r\nHost: [::1]\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    /// The macOS tripwire: a client that waits before sending its request
    /// still gets a real response, not a silently closed connection. See
    /// [`request_after_delay`] for why the delay is chosen the way it is.
    #[test]
    fn a_request_sent_after_a_delay_still_gets_the_response() {
        let root = artifact_root("e2e-delayed-write");
        let server = test_server(&root);
        let response = request_after_delay(
            server.local_addr(),
            Duration::from_millis(200),
            "GET /pkg/app_bg.wasm HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.ends_with("\0asm\u{1}\0\0\0"), "{response:?}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn head_returns_the_headers_without_the_body() {
        let root = artifact_root("e2e-head");
        let server = test_server(&root);
        let response = request(
            server.local_addr(),
            "HEAD /pkg/app_bg.wasm HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(response.contains("Content-Length: 8\r\n"), "{response}");
        assert!(response.ends_with("\r\n\r\n"), "{response:?}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_file_a_traversal_and_a_post_get_typed_statuses() {
        let root = artifact_root("e2e-errors");
        let server = test_server(&root);
        let addr = server.local_addr();
        assert!(
            request(addr, "GET /nope.js HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .starts_with("HTTP/1.1 404 ")
        );
        assert!(
            request(addr, "GET /../secret HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .starts_with("HTTP/1.1 403 ")
        );
        let post = request(
            addr,
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
        );
        assert!(post.starts_with("HTTP/1.1 405 "), "{post}");
        assert!(post.contains("Allow: GET, HEAD\r\n"), "{post}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    /// A refused request can still carry a body (a `POST`, since
    /// `read_request` never reads one), which leaves those bytes sitting
    /// unread in the socket's receive buffer at close time — the shape
    /// that makes a platform answer with an abortive reset instead of a
    /// graceful close, even though the response was already sent in full.
    /// Confirms the connection still closes cleanly; see
    /// [`finish_connection`]/[`DRAIN_TIMEOUT`].
    #[test]
    fn a_refused_request_with_an_unread_body_still_closes_cleanly() {
        let root = artifact_root("e2e-post-body-drain");
        let server = test_server(&root);
        let body = "x".repeat(4096);
        let response = request(
            server.local_addr(),
            &format!(
                "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            ),
        );
        assert!(response.starts_with("HTTP/1.1 405 "), "{response}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_request_log_sink_receives_one_line_per_request() {
        let root = artifact_root("e2e-log");
        let lines: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let server = serve(
            &root,
            ServeOptions {
                port: 0,
                ..ServeOptions::default()
            },
            Some(Arc::new(move |line: &str| {
                sink.lock().unwrap().push(line.to_string())
            })),
        )
        .unwrap();
        request(
            server.local_addr(),
            "GET /pkg/app.js HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        server.shutdown();
        let recorded = lines.lock().unwrap().clone();
        assert_eq!(recorded, vec!["GET /pkg/app.js -> 200".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    /// The scrubber's contract on the inputs it exists for: NUL, a terminal
    /// escape, the CR/LF that would forge a second log line, and DEL each
    /// become one space; printable text is untouched; an over-long field is
    /// cut at [`MAX_LOGGED_FIELD`] with an ellipsis.
    #[test]
    fn sanitize_log_field_replaces_every_control_character_and_caps_length() {
        let hostile = "/pkg/\x1b[2J\x00app\r\n.js\x7f";
        let clean = sanitize_log_field(hostile);
        assert!(!clean.chars().any(|c| c.is_ascii_control()), "{clean:?}");
        assert_eq!(clean, "/pkg/ [2J app  .js ");
        assert_eq!(sanitize_log_field("GET /pkg/app.js"), "GET /pkg/app.js");
        let long = "a".repeat(MAX_LOGGED_FIELD + 10);
        let cut = sanitize_log_field(&long);
        assert_eq!(cut.chars().count(), MAX_LOGGED_FIELD + 1);
        assert!(cut.ends_with('\u{2026}'), "{cut:?}");
        assert_eq!(
            sanitize_log_field(&"b".repeat(MAX_LOGGED_FIELD))
                .chars()
                .count(),
            MAX_LOGGED_FIELD,
            "a field exactly at the cap is not cut"
        );
    }

    /// End to end: a request line whose target carries a terminal escape
    /// and a NUL reaches the log sink with both scrubbed. Neither byte is
    /// whitespace, so the request-line split keeps them inside the target,
    /// and the path resolver answers 404 for the name they corrupt — the
    /// sink still sees exactly one line, with no control character in it.
    #[test]
    fn control_characters_in_the_request_line_never_reach_the_log_sink() {
        let root = artifact_root("e2e-sanitize");
        let lines: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let server = serve(
            &root,
            ServeOptions {
                port: 0,
                ..ServeOptions::default()
            },
            Some(Arc::new(move |line: &str| {
                sink.lock().unwrap().push(line.to_string())
            })),
        )
        .unwrap();
        // `\x1b[2J` clears a terminal; `\x00` truncates a C string.
        request(
            server.local_addr(),
            "GET /pkg/\x1b[2J\x00app.js HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        server.shutdown();
        let recorded = lines.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        let line = &recorded[0];
        assert!(!line.chars().any(|c| c.is_ascii_control()), "{line:?}");
        assert_eq!(line, "GET /pkg/ [2J app.js -> 404");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn serving_a_directory_that_is_not_there_is_a_typed_refusal() {
        let dir = temp_dir("no-root");
        let missing = dir.join("dist/web");
        assert!(matches!(
            serve(&missing, ServeOptions::default(), None),
            Err(ServeError::RootMissing { .. })
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Dropping the handle must free the port — a preview loop rebinds the
    /// same one on every restart.
    #[test]
    fn dropping_the_server_releases_the_port() {
        let root = artifact_root("rebind");
        let server = test_server(&root);
        let addr = server.local_addr();
        drop(server);
        let rebound = serve(
            &root,
            ServeOptions {
                host: addr.ip(),
                port: addr.port(),
            },
            None,
        )
        .expect("the port is free once the handle is dropped");
        assert_eq!(rebound.local_addr(), addr);
        rebound.shutdown();
        let _ = fs::remove_dir_all(&root);
    }
}
