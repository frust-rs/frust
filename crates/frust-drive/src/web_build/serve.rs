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
//! answers wrongly (`platform/web/README.md` records this), so a bad
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
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The port [`ServeOptions::default`] binds.
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
/// `wasm-bindgen --target web` output directory plus `platform/web`'s host
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
    };

    if let Some(log) = log {
        log(&format!(
            "{} {} -> {}",
            request.method,
            request.target,
            status.code()
        ));
    }
}

/// The request line's method and target, all this server reads. Headers are
/// consumed to find the end of the head and then discarded — nothing here
/// varies on one.
struct Request {
    method: String,
    target: String,
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
    let request = Request {
        method: method.to_string(),
        target: target.to_string(),
    };

    // Drain the remaining header lines up to the blank line terminating the
    // head, bounded by the same cap. A body (a POST this server refuses) is
    // never read: the response closes the connection regardless.
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

fn respond(
    writer: &mut TcpStream,
    status: Status,
    body: &[u8],
    mime: Option<&str>,
    head_only: bool,
) -> std::io::Result<()> {
    let mime = mime.unwrap_or("text/plain; charset=utf-8");
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n{NO_CACHE_HEADERS}",
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
/// component by component: a `..` segment is refused outright rather than
/// resolved, and a segment carrying a path separator (which can only appear
/// after percent-decoding — `%2f`, `%5c`) is refused for the same reason. That
/// is what keeps `GET /../../etc/passwd`, and its encoded spellings, from
/// reaching a byte outside the artifact directory. Nothing is canonicalized:
/// a symlink inside the artifact directory is the developer's own, and
/// resolving links would only move the decision to a place where the
/// component check no longer applies.
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
        if segment == ".."
            || segment.contains('/')
            || segment.contains('\\')
            || segment.contains('\0')
        {
            return Err(Status::Forbidden);
        }
        resolved.push(segment);
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
        assert!(response.ends_with("\0asm\u{1}\0\0\0"), "{response:?}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_root_url_serves_the_host_page() {
        let root = artifact_root("e2e-index");
        let server = test_server(&root);
        assert!(server.url().starts_with("http://127.0.0.1:"));
        let response = request(server.local_addr(), "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.ends_with("<!doctype html>"), "{response}");
        server.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn head_returns_the_headers_without_the_body() {
        let root = artifact_root("e2e-head");
        let server = test_server(&root);
        let response = request(
            server.local_addr(),
            "HEAD /pkg/app_bg.wasm HTTP/1.1\r\nHost: x\r\n\r\n",
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
            request(addr, "GET /nope.js HTTP/1.1\r\nHost: x\r\n\r\n").starts_with("HTTP/1.1 404 ")
        );
        assert!(
            request(addr, "GET /../secret HTTP/1.1\r\nHost: x\r\n\r\n")
                .starts_with("HTTP/1.1 403 ")
        );
        let post = request(
            addr,
            "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
        );
        assert!(post.starts_with("HTTP/1.1 405 "), "{post}");
        assert!(post.contains("Allow: GET, HEAD\r\n"), "{post}");
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
            "GET /pkg/app.js HTTP/1.1\r\nHost: x\r\n\r\n",
        );
        server.shutdown();
        let recorded = lines.lock().unwrap().clone();
        assert_eq!(recorded, vec!["GET /pkg/app.js -> 200".to_string()]);
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
