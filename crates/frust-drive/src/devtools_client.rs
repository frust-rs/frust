//! Blocking NDJSON client for the `frust-devtools` wire protocol
//! (`frust-devtools-protocol`) — the tool-side half of the connection
//! `frust-drive`/`frust-tui` use to inspect/drive a running Frust app.
//! Tooling-isolation charter (`docs/ARCHITECTURE.md`): `std::net::TcpStream`
//! plus a plain background reader thread only — no tokio, no framework
//! crate. Threading idioms mirror `crate::process`'s `LineReceiver`
//! (`Mutex`+`Condvar`, a dedicated reader thread, drop-oldest-under-load
//! semantics) rather than inventing a new shape for this crate.
//!
//! # Auth
//!
//! A devtools service mints a random per-process token and prints it on its
//! discovery line; every method but `handshake` is refused
//! ([`frust_devtools_protocol::RpcError::UNAUTHORIZED`]) until a connection
//! presents it. So the flow is: recover a
//! [`Discovery`](frust_devtools_protocol::Discovery) from the app's log/logcat
//! stream with `parse_discovery_line`, hand its `token` to
//! [`DevtoolsClient::connect`], and call [`DevtoolsClient::handshake`] before
//! anything else. A rejection surfaces as a [`DevtoolsRpcError`] carried
//! through `anyhow`, so a caller can tell "wrong/no token" ([`is_unauthorized`])
//! from an ordinary server-side failure instead of pattern-matching a string.
//!
//! # Threading model
//!
//! [`DevtoolsClient::connect`] opens a blocking [`TcpStream`] and spawns one
//! background reader thread that owns a cloned read half. Every typed call
//! (`handshake`, `widget_tree`, `tap`, …) writes a JSON-RPC request line on
//! the caller's thread, then blocks on a per-request one-shot channel for
//! the matching [`Response`] the reader thread routes to it by `id`. A
//! [`Notification`] never satisfies a pending call — the reader dispatches
//! it to whichever subscription (v1: only `frame_stats`) is armed instead.
//! The connection's read/write timeout (`connect`'s `timeout` argument) is
//! also each call's own wait bound, via `mpsc::Receiver::recv_timeout`. The
//! reader thread's own line-framing buffer survives a socket read timeout
//! (an ordinary idle tick, not a line boundary) rather than discarding
//! whatever was already read — see [`LineReader`] — and refuses to grow
//! past [`MAX_LINE_BYTES`], mirroring `frust-devtools::server`'s identical
//! server-side cap.
//!
//! # Frame-stats subscription (bounded, drop-oldest)
//!
//! [`DevtoolsClient::subscribe_frame_stats`] hands back a plain
//! `std::sync::mpsc::Receiver<FrameStats>`. The reader thread never blocks
//! on delivery: each new `frame_stats` notification overwrites a
//! single-slot mailbox (`FrameStatsMailbox` — a `Mutex<Option<FrameStats>>`
//! paired with a `Condvar`, the same shape as
//! `crate::process::LineBufferShared`) instead of pushing into an unbounded
//! queue. A small relay thread drains
//! that mailbox one value at a time into a capacity-1 `mpsc::sync_channel`.
//! If the caller falls behind, later notifications simply overwrite the
//! mailbox before the relay thread gets to it — the *oldest* unread sample
//! is what's discarded, never the newest, and memory use never grows with a
//! lagging consumer. Calling `subscribe_frame_stats` again replaces the
//! prior subscription (its relay thread exits once the caller drops the old
//! `Receiver`, which fails the relay's next `send`).
//!
//! # Hot patching
//!
//! [`DevtoolsClient::hotpatch_info`], [`DevtoolsClient::patch_chunk`] and
//! [`DevtoolsClient::apply_patch`] are the three hot-patch methods, gated
//! on [`Capability::HotPatch`]. A patch travels as base64 chunks of at most
//! [`PATCH_CHUNK_MAX_BYTES`] raw bytes ([`patch_chunks`], sent in order by
//! [`DevtoolsClient::upload_patch`]), so each request line stays under the
//! 1 MiB cap both ends enforce. `apply_patch` is answered only after the
//! app's next frame, so it waits at least [`APPLY_PATCH_MIN_WAIT`] whatever
//! the connection's own timeout.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use frust_devtools_protocol::{
    AckResult, ApplyPatchParams, Capability, FrameStats, HandshakeInfo, HandshakeParams,
    HotpatchInfo, Incoming, InputScrollParams, InputTapParams, InputTextParams, Method,
    MetricsSnapshot, PatchChunkParams, PatchOutcome, Request, Response, ResponseOutcome, RpcError,
    ScreenshotResult, WidgetProps, WidgetPropsParams, WidgetTreeDump, decode_line, encode_line,
};
use serde_json::Value;

use crate::process::ProcessRunner;

/// An RPC-level rejection: the server answered, and said no.
///
/// A `thiserror` enum would be the house style for a matched error, but the
/// server's code set is open-ended (JSON-RPC reserves a whole
/// implementation-defined range), so the code is carried as data with named
/// predicates over it rather than enumerated into variants that would go stale.
/// Returned inside `anyhow::Error`, so `err.downcast_ref::<DevtoolsRpcError>()`
/// (or [`is_unauthorized`]) recovers it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("devtools server rejected `{method}` (code {code}): {message}")]
pub struct DevtoolsRpcError {
    pub method: String,
    pub code: i32,
    pub message: String,
}

impl DevtoolsRpcError {
    /// The connection has not presented the app's devtools token. Retrying is
    /// pointless — re-read the discovery line and reconnect with its token.
    pub fn is_unauthorized(&self) -> bool {
        self.code == RpcError::UNAUTHORIZED
    }

    /// The server has no matching [`Capability`] declared at handshake for
    /// this method (e.g. `screenshot` on a backend defaulting to
    /// `NotSupported`). Distinct from [`is_unauthorized`](Self::is_unauthorized):
    /// the token was fine, the method just isn't offered — retrying won't
    /// help, and a caller with the handshake's capability set in hand
    /// ([`DevtoolsClient::capabilities`]) should generally check first rather
    /// than round-trip into this.
    pub fn is_not_supported(&self) -> bool {
        self.code == RpcError::NOT_SUPPORTED
    }
}

/// Whether `err` is a devtools rejection for want of a valid token — the one
/// failure a caller must act on differently (fix the token, don't retry).
pub fn is_unauthorized(err: &anyhow::Error) -> bool {
    err.downcast_ref::<DevtoolsRpcError>()
        .is_some_and(DevtoolsRpcError::is_unauthorized)
}

/// Whether `err` is a devtools rejection because the server declared no
/// matching capability at handshake — see
/// [`DevtoolsRpcError::is_not_supported`].
pub fn is_not_supported(err: &anyhow::Error) -> bool {
    err.downcast_ref::<DevtoolsRpcError>()
        .is_some_and(DevtoolsRpcError::is_not_supported)
}

/// Whether `err` is a devtools rejection because the server does not know
/// the method at all (`METHOD_NOT_FOUND`) — what a hot-patch method answers
/// on an app built without the capability.
pub fn is_method_not_found(err: &anyhow::Error) -> bool {
    err.downcast_ref::<DevtoolsRpcError>()
        .is_some_and(|rpc| rpc.code == RpcError::METHOD_NOT_FOUND)
}

/// The most raw patch bytes one `patch_chunk` carries — the app refuses a
/// larger chunk. 512 KiB is about 683 KiB as base64, well under the 1 MiB
/// line cap.
pub const PATCH_CHUNK_MAX_BYTES: usize = 512 * 1024;

/// The least time [`DevtoolsClient::apply_patch`] waits for its answer. The
/// app replies after the frame that follows the attempt and gives up waiting
/// for that frame after 5 s, so a shorter connection timeout would turn every
/// slow-but-successful apply into an unknown outcome.
pub const APPLY_PATCH_MIN_WAIT: Duration = Duration::from_secs(15);

/// `bytes` as standard, padded base64 (RFC 4648 §4) — the encoding
/// `patch_chunk` carries. Hand-rolled because this crate otherwise has no
/// use for a base64 dependency.
pub fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `bytes` split into the `patch_chunk` requests that carry them, in offset
/// order, each at most [`PATCH_CHUNK_MAX_BYTES`] raw.
pub fn patch_chunks(patch_id: u64, bytes: &[u8]) -> Vec<PatchChunkParams> {
    let total_len = bytes.len() as u64;
    bytes
        .chunks(PATCH_CHUNK_MAX_BYTES)
        .enumerate()
        .map(|(index, data)| PatchChunkParams {
            patch_id,
            offset: (index * PATCH_CHUNK_MAX_BYTES) as u64,
            total_len,
            data_base64: encode_base64(data),
        })
        .collect()
}

/// A blocking client over one devtools TCP connection. See the module doc
/// for the threading model. `Send + Sync`: every field synchronizes its own
/// access, so a caller may share one client (e.g. behind an `Arc`) across
/// threads without an external lock.
pub struct DevtoolsClient {
    write_stream: Mutex<TcpStream>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Response>>>>,
    frame_stats: Arc<FrameStatsMailbox>,
    timeout: Duration,
    /// The token this connection presents at `handshake`. Held (rather than
    /// taken per call) because it belongs to the *connection*: the server
    /// authenticates the socket once, and re-sending the secret on later
    /// requests would only widen its exposure.
    token: Option<String>,
    /// The capability set the last successful [`handshake`](Self::handshake)
    /// declared, cached so a caller can ask "does this app support X?" via
    /// [`capabilities`](Self::capabilities) without a second round trip —
    /// `None` until `handshake` succeeds at least once.
    capabilities: Mutex<Option<Vec<Capability>>>,
    reader: Option<thread::JoinHandle<()>>,
    /// Set by the reader thread when it closes the connection for a reason
    /// more specific than "nothing came back in time" (currently: an
    /// oversized line, see [`MAX_LINE_BYTES`]) — consulted by [`call`](Self::call)
    /// so that failure surfaces its actual cause instead of always reading
    /// as an ordinary timeout.
    close_reason: Arc<Mutex<Option<String>>>,
}

impl DevtoolsClient {
    /// Connects to a devtools server at `addr` (typically an `adb forward`ed
    /// or desktop-loopback port discovered via
    /// [`frust_devtools_protocol::parse_discovery_line`]). `timeout` is both
    /// the socket's read/write timeout and the bound every subsequent call
    /// waits for its response before failing.
    ///
    /// `token` is the one the same discovery line carried
    /// (`Discovery::token`), presented at [`handshake`](Self::handshake).
    /// `None` is for a server running with auth off — against a default
    /// (auth-on) service every other method then fails
    /// [`is_unauthorized`].
    pub fn connect(
        addr: impl ToSocketAddrs,
        timeout: Duration,
        token: Option<&str>,
    ) -> Result<Self> {
        let stream = TcpStream::connect(addr).context("failed to connect to devtools server")?;
        stream
            .set_read_timeout(Some(timeout))
            .context("failed to set devtools connection read timeout")?;
        stream
            .set_write_timeout(Some(timeout))
            .context("failed to set devtools connection write timeout")?;
        // Best-effort latency win (a devtools round trip is small, frequent
        // request/response traffic) — never load-bearing, so an unsupported
        // platform/socket just keeps Nagle's default behavior.
        let _ = stream.set_nodelay(true);
        let read_stream = stream
            .try_clone()
            .context("failed to clone the devtools connection for its reader thread")?;

        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let frame_stats = Arc::new(FrameStatsMailbox::new());
        let close_reason = Arc::new(Mutex::new(None));

        let reader_pending = Arc::clone(&pending);
        let reader_frame_stats = Arc::clone(&frame_stats);
        let reader_close_reason = Arc::clone(&close_reason);
        let reader = thread::spawn(move || {
            run_reader_loop(
                read_stream,
                reader_pending,
                reader_frame_stats,
                reader_close_reason,
            )
        });

        Ok(Self {
            write_stream: Mutex::new(stream),
            next_id: AtomicU64::new(1),
            pending,
            frame_stats,
            timeout,
            token: token.map(str::to_string),
            capabilities: Mutex::new(None),
            reader: Some(reader),
            close_reason,
        })
    }

    /// `handshake` — presents this connection's token (see
    /// [`connect`](Self::connect)) and returns the server's identity plus its
    /// declared capability set.
    ///
    /// **Call this first.** Until it succeeds the server refuses every other
    /// method; a rejection here is [`is_unauthorized`] and means the token is
    /// wrong or missing, not that the app is unreachable.
    pub fn handshake(&self) -> Result<HandshakeInfo> {
        let params = serde_json::to_value(HandshakeParams {
            token: self.token.clone(),
        })
        .context("encoding `handshake` params")?;
        let info: HandshakeInfo = self.typed_call(Method::Handshake, params)?;
        *self.capabilities.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(info.capabilities.clone());
        Ok(info)
    }

    /// The capability set the last successful [`handshake`](Self::handshake)
    /// declared, or `None` if `handshake` has not yet succeeded on this
    /// connection. Lets a caller (e.g. an MCP screenshot tool) check "does
    /// this app declare `Screenshot`?" against the cached handshake result
    /// rather than a doomed round trip that a `NOT_SUPPORTED` server would
    /// just reject.
    pub fn capabilities(&self) -> Option<Vec<Capability>> {
        self.capabilities
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// `widget_tree` — the current retained widget tree, nested root-down.
    pub fn widget_tree(&self) -> Result<WidgetTreeDump> {
        self.typed_call(Method::WidgetTree, Value::Null)
    }

    /// `widget_props` — the named widget's debug property entries.
    pub fn widget_props(&self, id: u64) -> Result<WidgetProps> {
        let params = serde_json::to_value(WidgetPropsParams { id })
            .context("encoding `widget_props` params")?;
        self.typed_call(Method::WidgetProps, params)
    }

    /// `metrics_snapshot` — process RSS (best-effort) and uptime.
    pub fn metrics_snapshot(&self) -> Result<MetricsSnapshot> {
        self.typed_call(Method::MetricsSnapshot, Value::Null)
    }

    /// `screenshot` — a base64-encoded PNG of the current frame. Capability-
    /// gated: a server whose handshake didn't declare
    /// [`Capability::Screenshot`] rejects this with
    /// [`RpcError::NOT_SUPPORTED`], detectable via
    /// [`DevtoolsRpcError::is_not_supported`]/[`is_not_supported`] — check
    /// [`capabilities`](Self::capabilities) first to avoid the round trip
    /// entirely when the answer is already known.
    pub fn screenshot(&self) -> Result<ScreenshotResult> {
        self.typed_call(Method::Screenshot, Value::Null)
    }

    /// `input_tap` — synthesizes a tap at logical-px `(x, y)`.
    pub fn tap(&self, x: f64, y: f64) -> Result<AckResult> {
        let params =
            serde_json::to_value(InputTapParams { x, y }).context("encoding `input_tap` params")?;
        self.typed_call(Method::InputTap, params)
    }

    /// `input_scroll` — synthesizes a scroll of `(dx, dy)` at logical-px
    /// `(x, y)`.
    pub fn scroll(&self, x: f64, y: f64, dx: f64, dy: f64) -> Result<AckResult> {
        let params = serde_json::to_value(InputScrollParams { x, y, dx, dy })
            .context("encoding `input_scroll` params")?;
        self.typed_call(Method::InputScroll, params)
    }

    /// `input_text` — synthesizes text input.
    pub fn text(&self, text: impl Into<String>) -> Result<AckResult> {
        let params = serde_json::to_value(InputTextParams { text: text.into() })
            .context("encoding `input_text` params")?;
        self.typed_call(Method::InputText, params)
    }

    /// `hotpatch_info` — what a patch builder needs to target this process
    /// (its runtime anchor and pid) and the app's patch counters, plus the
    /// layout-mismatch records the app has not reported yet. A server
    /// without [`Capability::HotPatch`] rejects it ([`is_not_supported`],
    /// its message naming the failed precondition, or
    /// [`is_method_not_found`] on a build without the feature).
    pub fn hotpatch_info(&self) -> Result<HotpatchInfo> {
        self.typed_call(Method::HotpatchInfo, Value::Null)
    }

    /// `patch_chunk` — one slice of a patch's bytes (see [`patch_chunks`]).
    pub fn patch_chunk(&self, params: &PatchChunkParams) -> Result<AckResult> {
        let params = serde_json::to_value(params).context("encoding `patch_chunk` params")?;
        self.typed_call(Method::PatchChunk, params)
    }

    /// Sends every chunk of `bytes` as `patch_id`, in order, stopping at the
    /// first failure. A chunk the server acknowledges with `ok: false` is an
    /// error too: the app would refuse the `apply_patch` that follows.
    pub fn upload_patch(&self, patch_id: u64, bytes: &[u8]) -> Result<()> {
        for chunk in patch_chunks(patch_id, bytes) {
            let ack = self.patch_chunk(&chunk)?;
            if !ack.ok {
                bail!(
                    "the app refused patch {patch_id}'s chunk at offset {}",
                    chunk.offset
                );
            }
        }
        Ok(())
    }

    /// `apply_patch` — applies the bytes previously uploaded as
    /// `params.patch_id`. Waits up to the connection timeout or
    /// [`APPLY_PATCH_MIN_WAIT`], whichever is longer, since the app answers
    /// only after its next frame. An `Err` that is not a
    /// [`DevtoolsRpcError`] (a closed connection, a timeout, an undecodable
    /// reply) leaves the outcome unknown: the patch may have been applied.
    pub fn apply_patch(&self, params: &ApplyPatchParams) -> Result<PatchOutcome> {
        let params = serde_json::to_value(params).context("encoding `apply_patch` params")?;
        let wait = self.timeout.max(APPLY_PATCH_MIN_WAIT);
        let result = self.call_waiting(Method::ApplyPatch, params, wait)?;
        serde_json::from_value(result).context("decoding `apply_patch` result")
    }

    /// The remote address this connection is open to, when the socket can
    /// still report it.
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.write_stream
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .peer_addr()
            .ok()
    }

    /// Arms the server's `frame_stats` push (`frame_stats_subscribe`) and
    /// returns a receiver for the notifications that follow. See the module
    /// doc for the bounded, drop-oldest delivery mechanism. Calling this
    /// again replaces the prior subscription.
    pub fn subscribe_frame_stats(&self) -> Result<mpsc::Receiver<FrameStats>> {
        let _ack: AckResult = self.typed_call(Method::FrameStatsSubscribe, Value::Null)?;
        Ok(self.frame_stats.subscribe())
    }

    /// Sends `method`/`params`, waits for the matching response, and
    /// decodes its `result` as `T`.
    fn typed_call<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        params: Value,
    ) -> Result<T> {
        let result = self.call(method, params)?;
        serde_json::from_value(result).with_context(|| format!("decoding `{method}` result"))
    }

    /// Sends one JSON-RPC request and blocks (up to `self.timeout`) for its
    /// response, returning the raw `result` value on success or an `Err` for
    /// an RPC-level error, a decode failure, or a timeout.
    fn call(&self, method: Method, params: Value) -> Result<Value> {
        self.call_waiting(method, params, self.timeout)
    }

    /// [`call`](Self::call) waiting up to `wait` for the response.
    fn call_waiting(&self, method: Method, params: Value, wait: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel::<Response>();
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);

        let request = Request::new(id, method.as_str(), params);
        let line = format!("{}\n", encode_line(&request));
        {
            let mut stream = self.write_stream.lock().unwrap_or_else(|p| p.into_inner());
            stream
                .write_all(line.as_bytes())
                .with_context(|| format!("failed to send a `{method}` request"))?;
            stream
                .flush()
                .with_context(|| format!("failed to flush a `{method}` request"))?;
        }

        let response = rx.recv_timeout(wait).map_err(|err| {
            // Not coming — stop the reader thread from ever routing a late
            // response into a channel nobody is listening on any more.
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            match err {
                // The reader thread dropped every pending sender (see
                // `run_reader_loop`) because the connection is gone. If it
                // closed for a specific, known reason (currently: an
                // oversized line — see `MAX_LINE_BYTES`) surface that
                // instead of the generic, misleading "timed out".
                mpsc::RecvTimeoutError::Disconnected => {
                    match self
                        .close_reason
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone()
                    {
                        Some(reason) => anyhow!(
                            "the devtools connection closed while waiting for a `{method}` response: {reason}"
                        ),
                        None => anyhow!(
                            "the devtools connection closed while waiting for a `{method}` response"
                        ),
                    }
                }
                mpsc::RecvTimeoutError::Timeout => {
                    anyhow!("timed out waiting for a `{method}` response after {wait:?}")
                }
            }
        })?;

        match response.outcome {
            ResponseOutcome::Success { result } => Ok(result),
            // A typed error, not a formatted string: `UNAUTHORIZED` is the one
            // failure a caller has to handle differently, and it must survive
            // the trip through `anyhow` intact (see [`is_unauthorized`]).
            ResponseOutcome::Error { error } => Err(DevtoolsRpcError {
                method: method.to_string(),
                code: error.code,
                message: error.message,
            }
            .into()),
        }
    }
}

impl Drop for DevtoolsClient {
    fn drop(&mut self) {
        // Shutting down the socket (shared with the reader thread's cloned
        // handle — see `connect`) unblocks its read loop promptly instead of
        // waiting up to a full read-timeout tick.
        if let Ok(stream) = self.write_stream.lock() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// A single line from the peer longer than this without a newline is
/// treated as a broken connection and closes it. Mirrors
/// `frust_devtools::server::MAX_LINE_BYTES` (`crates/frust-devtools/src/server.rs`)
/// exactly — the two ends of the wire must agree on what "too big" means, and
/// nothing in the v1 protocol's server→client direction (a response or a
/// `frame_stats` notification) comes close to it.
const MAX_LINE_BYTES: usize = 1 << 20;

/// Read chunk size [`LineReader`] asks the OS for per `read()` call. Also
/// mirrors the server's own `READ_CHUNK_BYTES` — not load-bearing on its
/// own, just keeps the two sides' framing code shaped alike.
const READ_CHUNK_BYTES: usize = 4096;

/// The reader thread's NDJSON line framing over one blocking, read-timeout'd
/// [`TcpStream`]. Hand-rolled (explicit chunked `Read::read` calls into an
/// owned `Vec<u8>`) rather than `BufRead::read_until`, for the same reason
/// `frust_devtools::server`'s own `LineReader` is hand-rolled: the
/// [`MAX_LINE_BYTES`] cap has to be enforced *while* accumulating, not after
/// — `read_until` only ever returns control once it finds the delimiter or
/// hits a real I/O error, so a peer that never sends a `\n` would grow this
/// buffer without bound before any cap check ran.
///
/// The accumulation buffer persists across [`next_line`](Self::next_line)
/// calls that return [`ReadOutcome::WouldBlock`] — a socket read timeout is
/// this connection's ordinary idle tick (`connect` sets one), not a line
/// boundary, so whatever was already read for the in-progress line stays
/// buffered for the next call rather than being thrown away. A complete
/// line decodes via `String::from_utf8_lossy`, matching the server's own
/// choice: a byte-cut multibyte UTF-8 sequence at a chunk/timeout boundary
/// substitutes rather than erroring or (as `BufRead::read_line` would)
/// silently discarding the bytes already read.
struct LineReader {
    stream: TcpStream,
    buf: Vec<u8>,
}

/// One [`LineReader::next_line`] call's outcome.
enum ReadOutcome {
    /// A complete line, terminator stripped.
    Line(String),
    /// The read timed out with no complete line buffered yet — the
    /// connection's ordinary idle tick. Nothing was discarded; call again.
    WouldBlock,
    /// The peer closed the connection (`read` returned `Ok(0)`), possibly
    /// after an unterminated trailing partial line — under NDJSON framing
    /// that partial is not a line and nothing more is coming for it.
    Eof,
    /// A real I/O error tore down the stream.
    Err(std::io::Error),
    /// The buffered line grew past [`MAX_LINE_BYTES`] without a newline.
    Oversized,
}

impl LineReader {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            buf: Vec::with_capacity(READ_CHUNK_BYTES),
        }
    }

    fn next_line(&mut self) -> ReadOutcome {
        loop {
            if let Some(newline) = self.buf.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=newline).collect();
                line.pop(); // the '\n'
                if line.last() == Some(&b'\r') {
                    line.pop(); // tolerate CRLF from a hand-driven peer
                }
                return ReadOutcome::Line(String::from_utf8_lossy(&line).into_owned());
            }
            if self.buf.len() > MAX_LINE_BYTES {
                self.buf.clear();
                return ReadOutcome::Oversized;
            }

            let mut chunk = [0u8; READ_CHUNK_BYTES];
            match self.stream.read(&mut chunk) {
                Ok(0) => return ReadOutcome::Eof,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return ReadOutcome::WouldBlock;
                }
                Err(e) => return ReadOutcome::Err(e),
            }
        }
    }
}

/// The background reader thread's body: decodes NDJSON lines off `stream`
/// until it closes, routing each [`Incoming::Response`] to its matching
/// pending call and each `frame_stats` [`Incoming::Notification`] into
/// `frame_stats`'s mailbox. A v1 server never sends
/// [`Incoming::Request`]s — tolerated as a silent no-op rather than treated
/// as a protocol error, since dropping the whole connection over one
/// unexpected shape would be a worse failure mode than ignoring it.
fn run_reader_loop(
    stream: TcpStream,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Response>>>>,
    frame_stats: Arc<FrameStatsMailbox>,
    close_reason: Arc<Mutex<Option<String>>>,
) {
    let mut reader = LineReader::new(stream);
    loop {
        match reader.next_line() {
            ReadOutcome::Eof => break, // The peer closed the connection.
            ReadOutcome::WouldBlock => {
                // The socket read timeout elapsed with nothing new to
                // read — an ordinary idle tick, not a connection failure.
                // Whatever was already buffered for an in-progress line is
                // still there (see `LineReader`'s doc); a real close
                // surfaces as `Eof`/`Err` above/below instead.
                continue;
            }
            ReadOutcome::Err(e) => {
                // A real I/O error: the stream is dead. Recorded for the
                // same reason as `Oversized` below — a caller mid-`call`
                // should see why the connection went away rather than a
                // generic "timed out".
                *close_reason.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some(format!("a read error tore down the connection: {e}"));
                break;
            }
            ReadOutcome::Oversized => {
                *close_reason.lock().unwrap_or_else(|p| p.into_inner()) = Some(format!(
                    "the peer sent a line exceeding {MAX_LINE_BYTES} bytes without a newline"
                ));
                break;
            }
            ReadOutcome::Line(line) => {
                if line.is_empty() {
                    continue;
                }
                match decode_line(&line) {
                    Ok(Incoming::Response(response)) => {
                        if let Some(tx) = pending
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .remove(&response.id)
                        {
                            let _ = tx.send(response);
                        }
                    }
                    Ok(Incoming::Notification(notification)) => {
                        if notification.method == Method::FrameStats.as_str()
                            && let Ok(stats) =
                                serde_json::from_value::<FrameStats>(notification.params)
                        {
                            frame_stats.publish(stats);
                        }
                        // Any other/unrecognized notification is silently
                        // ignored — v1 only routes `frame_stats` pushes.
                    }
                    Ok(Incoming::Request(_)) => {}
                    Err(_) => {
                        // A malformed line from the peer isn't fatal to the
                        // whole connection — keep reading rather than
                        // tearing down every pending call over one bad line.
                    }
                }
            }
        }
    }
    // The connection is gone: no `Response` will ever arrive for any call
    // still waiting. Drop every pending sender so those calls fail as soon
    // as the reader notices, instead of always burning their full timeout.
    pending.lock().unwrap_or_else(|p| p.into_inner()).clear();
    frame_stats.close();
}

/// Depth of the `mpsc::sync_channel` [`FrameStatsMailbox::subscribe`] relays
/// into. `1` is deliberate, not a placeholder to widen later: the mailbox
/// itself is already the buffer (see the module doc's drop-oldest
/// mechanism), so a deeper channel would only let more *stale* samples pile
/// up unread instead of collapsing to the latest one.
const FRAME_STATS_CHANNEL_CAP: usize = 1;

/// The single-slot, overwrite-on-publish mailbox backing
/// [`DevtoolsClient::subscribe_frame_stats`]'s drop-oldest delivery — see
/// the module doc. Shape mirrors `crate::process::LineBufferShared`
/// (`Mutex`+`Condvar` over a producer that never blocks), specialized to
/// hold at most one pending value instead of a bounded queue.
struct FrameStatsMailbox {
    state: Mutex<FrameStatsMailboxState>,
    ready: Condvar,
}

struct FrameStatsMailboxState {
    latest: Option<FrameStats>,
    closed: bool,
}

impl FrameStatsMailbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(FrameStatsMailboxState {
                latest: None,
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    /// Producer side (the reader thread): overwrites the mailbox. Never
    /// blocks — if a prior value hasn't been relayed yet, it is silently
    /// discarded. This is the "reader drops oldest" mechanism: only the
    /// most recently published value ever reaches
    /// [`subscribe`](Self::subscribe)'s relay thread.
    fn publish(&self, stats: FrameStats) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.latest = Some(stats);
        drop(state);
        self.ready.notify_one();
    }

    /// Marks the mailbox closed (the connection is gone) and wakes any
    /// relay thread blocked waiting for a value, so it can exit instead of
    /// waiting forever.
    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        drop(state);
        self.ready.notify_all();
    }

    /// Consumer side: spawns a relay thread pumping the mailbox into a
    /// fresh capacity-[`FRAME_STATS_CHANNEL_CAP`] `mpsc::sync_channel`, and
    /// returns its receiving half. Replaces any prior subscription — v1
    /// supports exactly one active `frame_stats` consumer per client,
    /// matching the wire protocol's single-arm `frame_stats_subscribe`
    /// contract; the relay thread backing a prior `Receiver` a caller has
    /// since dropped exits on its next blocked `send`.
    fn subscribe(self: &Arc<Self>) -> mpsc::Receiver<FrameStats> {
        let (tx, rx) = mpsc::sync_channel::<FrameStats>(FRAME_STATS_CHANNEL_CAP);
        let mailbox = Arc::clone(self);
        thread::spawn(move || {
            loop {
                let stats = {
                    let mut state = mailbox.state.lock().unwrap_or_else(|p| p.into_inner());
                    loop {
                        if let Some(stats) = state.latest.take() {
                            break stats;
                        }
                        if state.closed {
                            return;
                        }
                        state = mailbox.ready.wait(state).unwrap_or_else(|p| p.into_inner());
                    }
                };
                // Blocks only if the caller hasn't drained the previous
                // sample yet; unblocks the instant they call `recv`. A
                // dropped `Receiver` fails this send — nothing left to
                // relay to, so exit.
                if tx.send(stats).is_err() {
                    return;
                }
            }
        });
        rx
    }
}

/// Runs `adb -s <serial> forward tcp:0 tcp:<device_port>`, which asks `adb`
/// to allocate an ephemeral local port and forward it to `device_port` on
/// the device — the standard way to reach an Android app's devtools TCP
/// listener (bound to `localhost` on-device, per the devtools service's own
/// charter) from the host. Returns the allocated local port, parsed from
/// `adb`'s stdout (a bare decimal port number for a `tcp:0` request).
pub fn adb_forward_ephemeral(
    runner: &dyn ProcessRunner,
    serial: &str,
    device_port: u16,
) -> Result<u16> {
    let device_spec = format!("tcp:{device_port}");
    let out = runner.run("adb", &["-s", serial, "forward", "tcp:0", &device_spec])?;
    if !out.success {
        bail!(
            "`adb -s {serial} forward tcp:0 {device_spec}` failed: {}",
            out.stderr.trim()
        );
    }
    parse_forward_port(&out.stdout).ok_or_else(|| {
        anyhow!(
            "`adb -s {serial} forward tcp:0 {device_spec}` did not report an allocated port: {:?}",
            out.stdout
        )
    })
}

/// Removes a forward previously allocated by [`adb_forward_ephemeral`]:
/// `adb -s <serial> forward --remove tcp:<local_port>`.
pub fn adb_forward_remove(runner: &dyn ProcessRunner, serial: &str, local_port: u16) -> Result<()> {
    let local_spec = format!("tcp:{local_port}");
    let out = runner.run("adb", &["-s", serial, "forward", "--remove", &local_spec])?;
    if !out.success {
        bail!(
            "`adb -s {serial} forward --remove {local_spec}` failed: {}",
            out.stderr.trim()
        );
    }
    Ok(())
}

/// Parses the allocated port `adb forward tcp:0 <spec>` prints on stdout —
/// a bare decimal number on its own line, per `adb`'s own documented
/// behavior for an ephemeral (`tcp:0`) local forward.
fn parse_forward_port(stdout: &str) -> Option<u16> {
    stdout.trim().lines().next()?.trim().parse().ok()
}

/// The hand-rolled fake devtools server the client tests (and
/// `hotpatch::session`'s) drive, scripted per test.
#[cfg(test)]
pub(crate) mod test_server {
    use std::collections::{HashMap, VecDeque};
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use frust_devtools_protocol::{
        AckResult, ApplyPatchParams, Capability, FrameStats, HandshakeInfo, HandshakeParams,
        HotpatchInfo, Notification, PROTOCOL_VERSION, PatchChunkParams, PatchOutcome, RectPx,
        Request, Response, RpcError, ScreenshotResult, WidgetNode, WidgetTreeDump, encode_line,
    };

    use super::PATCH_CHUNK_MAX_BYTES;

    /// The token the fake requires at handshake — the stand-in for one
    /// recovered from a discovery line.
    pub(crate) const FAKE_TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// How the fake answers one `apply_patch`.
    #[derive(Debug, Clone)]
    pub(crate) enum ApplyReply {
        /// Answer with this outcome; `hotpatch_info`'s counters follow it.
        Outcome(PatchOutcome),
        /// Answer with this RPC error.
        Error(RpcError),
        /// Answer with this outcome after sleeping this long (an app
        /// waiting for its next frame).
        Delayed(std::time::Duration, PatchOutcome),
        /// Close the connection without answering: the reply is lost.
        Hangup,
    }

    /// What the fake declares and how it answers the hot-patch methods.
    #[derive(Debug, Clone, Default)]
    pub(crate) struct Script {
        pub capabilities: Vec<Capability>,
        /// The `hotpatch_info` answer (its pending list is replaced per call
        /// from `pending`).
        pub info: Option<HotpatchInfo>,
        /// `pending_layout_mismatches` for successive `hotpatch_info` calls;
        /// empty once exhausted.
        pub pending: VecDeque<Vec<String>>,
        /// Answers for successive `apply_patch` calls; an unscripted one
        /// hangs up.
        pub applies: VecDeque<ApplyReply>,
        /// The `NOT_SUPPORTED` message `hotpatch_info` answers without
        /// [`Capability::HotPatch`].
        pub unsupported_reason: String,
    }

    /// A running fake: its address plus what it received.
    pub(crate) struct FakeServer {
        pub addr: SocketAddr,
        log: Arc<Mutex<Vec<Request>>>,
        uploads: Arc<Mutex<HashMap<u64, Vec<u8>>>>,
        _handle: thread::JoinHandle<()>,
    }

    impl FakeServer {
        /// Every request method received, in order.
        pub fn methods(&self) -> Vec<String> {
            self.requests().into_iter().map(|r| r.method).collect()
        }

        /// Every request received, in order.
        pub fn requests(&self) -> Vec<Request> {
            self.log.lock().unwrap().clone()
        }

        /// The bytes reassembled for `patch_id`.
        pub fn uploaded(&self, patch_id: u64) -> Option<Vec<u8>> {
            self.uploads.lock().unwrap().get(&patch_id).cloned()
        }
    }

    /// Accepts one connection and answers it per `script`, enforcing the
    /// same auth gate the real service does (`handshake` with [`FAKE_TOKEN`]
    /// first, `UNAUTHORIZED` for anything else until then) and the same
    /// capability gates: `screenshot` is `NOT_SUPPORTED` without
    /// [`Capability::Screenshot`], `hotpatch_info` `NOT_SUPPORTED` and the
    /// patch methods `METHOD_NOT_FOUND` without [`Capability::HotPatch`].
    /// Chunks must arrive in offset order and hold at most
    /// [`PATCH_CHUNK_MAX_BYTES`] raw bytes; `apply_patch`'s `len` must
    /// equal the bytes received. A `frame_stats_subscribe` ack is followed
    /// by one `frame_stats` notification. Deliberately reimplements just
    /// enough JSON-RPC framing to drive the client end-to-end without
    /// depending on the in-app devtools service — this crate may depend on
    /// `frust-devtools-protocol` only (`docs/ARCHITECTURE.md`'s
    /// tooling-isolation charter).
    pub(crate) fn spawn(script: Script) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let uploads = Arc::new(Mutex::new(HashMap::new()));
        let (server_log, server_uploads) = (Arc::clone(&log), Arc::clone(&uploads));
        let handle = thread::spawn(move || serve(listener, script, server_log, server_uploads));
        FakeServer {
            addr,
            log,
            uploads,
            _handle: handle,
        }
    }

    fn serve(
        listener: TcpListener,
        mut script: Script,
        log: Arc<Mutex<Vec<Request>>>,
        uploads: Arc<Mutex<HashMap<u64, Vec<u8>>>>,
    ) {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let mut authenticated = false;
        let hot = script.capabilities.contains(&Capability::HotPatch);
        loop {
            line.clear();
            let n = reader.read_line(&mut line).unwrap_or(0);
            if n == 0 {
                break;
            }
            let Ok(req) = serde_json::from_str::<Request>(line.trim_end()) else {
                continue;
            };
            log.lock().unwrap().push(req.clone());

            if req.method == "handshake" {
                let presented = serde_json::from_value::<HandshakeParams>(req.params.clone())
                    .ok()
                    .and_then(|p| p.token);
                authenticated = presented.as_deref() == Some(FAKE_TOKEN);
            }
            let reply = if !authenticated {
                Err(RpcError::unauthorized(
                    "present the devtools token at handshake",
                ))
            } else {
                match req.method.as_str() {
                    "handshake" => Ok(serde_json::to_value(HandshakeInfo {
                        app_name: "fake-app".into(),
                        frust_version: "0.0.0".into(),
                        protocol_version: PROTOCOL_VERSION,
                        capabilities: script.capabilities.clone(),
                    })
                    .unwrap()),
                    "screenshot" if !script.capabilities.contains(&Capability::Screenshot) => Err(
                        RpcError::not_supported("this backend declares no Screenshot capability"),
                    ),
                    "hotpatch_info" if !hot => {
                        Err(RpcError::not_supported(script.unsupported_reason.clone()))
                    }
                    "patch_chunk" | "apply_patch" if !hot => Err(RpcError::new(
                        RpcError::METHOD_NOT_FOUND,
                        format!("unknown method `{}`", req.method),
                    )),
                    "hotpatch_info" => {
                        let mut info = script.info.clone().expect("a scripted hotpatch_info");
                        info.pending_layout_mismatches =
                            script.pending.pop_front().unwrap_or_default();
                        Ok(serde_json::to_value(info).unwrap())
                    }
                    "patch_chunk" => chunk(&req, &uploads),
                    "apply_patch" => match apply(&req, &uploads, &mut script) {
                        Some(reply) => reply,
                        None => break,
                    },
                    "widget_tree" => Ok(serde_json::to_value(WidgetTreeDump {
                        roots: vec![WidgetNode {
                            id: 1,
                            type_name: "Root".into(),
                            debug_label: None,
                            bounds: Some(RectPx {
                                x: 0.0,
                                y: 0.0,
                                width: 100.0,
                                height: 200.0,
                            }),
                            children: vec![],
                        }],
                    })
                    .unwrap()),
                    "screenshot" => Ok(serde_json::to_value(ScreenshotResult {
                        png_base64: "ZmFrZS1wbmc=".into(),
                    })
                    .unwrap()),
                    _ => Ok(serde_json::to_value(AckResult { ok: true }).unwrap()),
                }
            };
            let response = match reply {
                Ok(result) => Response::success(req.id, result),
                Err(error) => Response::error(req.id, error),
            };
            writeln!(writer, "{}", encode_line(&response)).unwrap();

            if authenticated && req.method == "frame_stats_subscribe" {
                let notif = Notification::new(
                    "frame_stats",
                    serde_json::to_value(FrameStats {
                        n: 1,
                        total_us: 1_000,
                        rebuild_us: 100,
                        layout_us: 100,
                        paint_us: 100,
                        encode_us: 100,
                        acquire_us: 100,
                        submit_us: 100,
                        skipped: false,
                    })
                    .unwrap(),
                );
                writeln!(writer, "{}", encode_line(&notif)).unwrap();
            }
        }
    }

    fn chunk(
        req: &Request,
        uploads: &Mutex<HashMap<u64, Vec<u8>>>,
    ) -> Result<serde_json::Value, RpcError> {
        let params: PatchChunkParams = serde_json::from_value(req.params.clone())
            .map_err(|e| invalid_params(e.to_string()))?;
        let data = decode_base64(&params.data_base64)
            .ok_or_else(|| invalid_params("patch chunk is not base64"))?;
        if data.len() > PATCH_CHUNK_MAX_BYTES {
            return Err(invalid_params("patch chunk over 512 KiB"));
        }
        let mut uploads = uploads.lock().unwrap();
        let bytes = uploads.entry(params.patch_id).or_default();
        if params.offset != bytes.len() as u64 {
            return Err(invalid_params("patch chunk out of order"));
        }
        bytes.extend_from_slice(&data);
        if bytes.len() as u64 > params.total_len {
            return Err(invalid_params("patch chunks exceed total_len"));
        }
        Ok(serde_json::to_value(AckResult { ok: true }).unwrap())
    }

    /// `None` hangs up.
    fn apply(
        req: &Request,
        uploads: &Mutex<HashMap<u64, Vec<u8>>>,
        script: &mut Script,
    ) -> Option<Result<serde_json::Value, RpcError>> {
        let params: ApplyPatchParams = match serde_json::from_value(req.params.clone()) {
            Ok(params) => params,
            Err(e) => return Some(Err(invalid_params(e.to_string()))),
        };
        let received = uploads
            .lock()
            .unwrap()
            .get(&params.patch_id)
            .map_or(0, |bytes| bytes.len() as u64);
        if received != params.len {
            return Some(Err(invalid_params(format!(
                "patch is {received} bytes, apply_patch says {}",
                params.len
            ))));
        }
        match script.applies.pop_front().unwrap_or(ApplyReply::Hangup) {
            ApplyReply::Outcome(outcome) => {
                if let Some(info) = script.info.as_mut() {
                    info.patches_applied = outcome.patches_applied;
                    info.patch_bytes_loaded = outcome.patch_bytes_loaded;
                }
                Some(Ok(serde_json::to_value(outcome).unwrap()))
            }
            ApplyReply::Delayed(wait, outcome) => {
                thread::sleep(wait);
                Some(Ok(serde_json::to_value(outcome).unwrap()))
            }
            ApplyReply::Error(error) => Some(Err(error)),
            ApplyReply::Hangup => None,
        }
    }

    fn invalid_params(message: impl Into<String>) -> RpcError {
        RpcError::new(RpcError::INVALID_PARAMS, message)
    }

    /// Standard padded base64 back to bytes; `None` for anything else.
    pub(crate) fn decode_base64(text: &str) -> Option<Vec<u8>> {
        let value = |c: u8| -> Option<u32> {
            Some(match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => return None,
            } as u32)
        };
        let bytes = text.as_bytes();
        if !bytes.len().is_multiple_of(4) {
            return None;
        }
        let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
        for quad in bytes.chunks(4) {
            let pad = quad.iter().rev().take_while(|c| **c == b'=').count();
            let mut n = 0u32;
            for (i, c) in quad.iter().enumerate() {
                n <<= 6;
                if i < 4 - pad {
                    n |= value(*c)?;
                }
            }
            let decoded = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
            out.extend_from_slice(&decoded[..3 - pad]);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{self, ApplyReply, FAKE_TOKEN, FakeServer, Script};
    use super::*;
    use crate::process::{FakeProcessRunner, Output};
    use frust_devtools_protocol::{
        ApplyPatchParams, Capability, HotpatchInfo, JumpTableWire, MissedKey, PROTOCOL_VERSION,
        PatchOutcome, WidgetNode,
    };
    use std::net::{SocketAddr, TcpListener};
    use std::time::Instant;

    /// [`test_server::spawn`] declaring the read-only capabilities.
    fn spawn_fake_server() -> (SocketAddr, FakeServer) {
        spawn_fake_server_with_capabilities(vec![
            Capability::WidgetTree,
            Capability::FrameStats,
            Capability::Input,
        ])
    }

    /// Like [`spawn_fake_server`], but the handshake declares exactly
    /// `capabilities` — lets a test control whether `screenshot` should
    /// succeed or answer `NOT_SUPPORTED`.
    fn spawn_fake_server_with_capabilities(
        capabilities: Vec<Capability>,
    ) -> (SocketAddr, FakeServer) {
        let server = test_server::spawn(Script {
            capabilities,
            ..Script::default()
        });
        (server.addr, server)
    }

    #[test]
    fn loopback_handshake_widget_tree_tap_and_frame_stats_subscription() {
        let (addr, _server) = spawn_fake_server();
        let client =
            DevtoolsClient::connect(addr, Duration::from_secs(2), Some(FAKE_TOKEN)).unwrap();

        let handshake = client.handshake().unwrap();
        assert_eq!(handshake.app_name, "fake-app");
        assert_eq!(handshake.protocol_version, PROTOCOL_VERSION);
        assert!(handshake.capabilities.contains(&Capability::WidgetTree));

        let tree = client.widget_tree().unwrap();
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].type_name, "Root");

        let ack = client.tap(10.0, 20.0).unwrap();
        assert!(ack.ok);

        let frame_stats_rx = client.subscribe_frame_stats().unwrap();
        let stats = frame_stats_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("a frame_stats notification should arrive on the subscription receiver");
        assert_eq!(stats.n, 1);
        assert_eq!(stats.total_us, 1_000);
    }

    #[test]
    fn capabilities_are_reachable_after_handshake_without_a_second_round_trip() {
        let (addr, _server) = spawn_fake_server_with_capabilities(vec![
            Capability::WidgetTree,
            Capability::Screenshot,
        ]);
        let client =
            DevtoolsClient::connect(addr, Duration::from_secs(2), Some(FAKE_TOKEN)).unwrap();

        // No cached capabilities before the first handshake.
        assert_eq!(client.capabilities(), None);

        let handshake = client.handshake().unwrap();
        assert_eq!(client.capabilities(), Some(handshake.capabilities.clone()));
        assert!(
            client
                .capabilities()
                .unwrap()
                .contains(&Capability::Screenshot)
        );
    }

    #[test]
    fn screenshot_round_trips_against_a_capable_server() {
        let (addr, _server) = spawn_fake_server_with_capabilities(vec![Capability::Screenshot]);
        let client =
            DevtoolsClient::connect(addr, Duration::from_secs(2), Some(FAKE_TOKEN)).unwrap();
        client.handshake().unwrap();

        let shot = client.screenshot().unwrap();
        assert_eq!(shot.png_base64, "ZmFrZS1wbmc=");
    }

    #[test]
    fn screenshot_not_supported_surfaces_as_a_typed_detectable_error() {
        let (addr, _server) = spawn_fake_server_with_capabilities(vec![Capability::WidgetTree]);
        let client =
            DevtoolsClient::connect(addr, Duration::from_secs(2), Some(FAKE_TOKEN)).unwrap();
        let handshake = client.handshake().unwrap();
        assert!(!handshake.capabilities.contains(&Capability::Screenshot));

        let err = client.screenshot().unwrap_err();
        assert!(
            is_not_supported(&err),
            "a capability-gated rejection must be distinguishable from any other failure: {err}"
        );
        assert_eq!(
            err.downcast_ref::<DevtoolsRpcError>().map(|e| e.code),
            Some(RpcError::NOT_SUPPORTED)
        );
        // And not confusable with the unrelated auth failure code.
        assert!(!is_unauthorized(&err));
    }

    #[test]
    fn a_wrong_token_is_surfaced_as_unauthorized_distinctly() {
        let (addr, _server) = spawn_fake_server();
        let client =
            DevtoolsClient::connect(addr, Duration::from_secs(2), Some("not-the-token")).unwrap();

        let err = client.handshake().unwrap_err();
        assert!(
            is_unauthorized(&err),
            "a bad token must be distinguishable from any other failure: {err}"
        );
        assert_eq!(
            err.downcast_ref::<DevtoolsRpcError>().map(|e| e.code),
            Some(RpcError::UNAUTHORIZED)
        );

        // And the gate really holds: the connection stays useless afterwards.
        assert!(is_unauthorized(&client.widget_tree().unwrap_err()));
    }

    #[test]
    fn no_token_at_all_is_unauthorized_too_and_other_errors_are_not() {
        let (addr, _server) = spawn_fake_server();
        let client = DevtoolsClient::connect(addr, Duration::from_secs(2), None).unwrap();
        assert!(is_unauthorized(&client.handshake().unwrap_err()));

        // A non-auth rejection must NOT read as unauthorized — otherwise a
        // caller would go re-reading discovery lines over an unrelated fault.
        let other = anyhow!(DevtoolsRpcError {
            method: "widget_props".to_string(),
            code: RpcError::INVALID_PARAMS,
            message: "no widget with id 9".to_string(),
        });
        assert!(!is_unauthorized(&other));
        assert!(!is_unauthorized(&anyhow!("a plain transport failure")));
    }

    #[test]
    fn read_timeout_path_when_server_never_replies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = thread::spawn(move || {
            // Accept the connection, then never write anything back — the
            // client's call must time out rather than hang.
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            thread::sleep(Duration::from_secs(5));
            drop(stream);
        });

        let client =
            DevtoolsClient::connect(addr, Duration::from_millis(200), Some(FAKE_TOKEN)).unwrap();
        let start = Instant::now();
        let err = client.handshake().unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "a timed-out call should return promptly, took {:?}",
            start.elapsed()
        );
        assert!(
            err.to_string().contains("timed out"),
            "expected a timeout error, got: {err}"
        );
    }

    /// Drives [`run_reader_loop`] directly against a raw, hand-timed server
    /// stream rather than through a full [`DevtoolsClient`] — the fix under
    /// test lives entirely in the reader thread's own framing, and going
    /// through `DevtoolsClient::call` would couple this test's write timing
    /// to `call`'s own response-wait bound (`connect`'s `timeout` doubles as
    /// both the socket's read timeout *and* every call's wait bound — see
    /// the module doc), which a deliberately-delayed write would then also
    /// trip. Driving the reader loop directly sidesteps that coupling and
    /// tests exactly the mechanism the fix changed. Returns the `Response`
    /// [`run_reader_loop`] routed to `id`'s slot, plus whatever
    /// `close_reason` it recorded (if it closed the connection).
    fn drive_reader_loop_for_one_response(
        stream: TcpStream,
        read_timeout: Duration,
        id: u64,
    ) -> Result<Response, mpsc::RecvTimeoutError> {
        stream.set_read_timeout(Some(read_timeout)).unwrap();
        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::channel();
        pending.lock().unwrap().insert(id, tx);
        let frame_stats = Arc::new(FrameStatsMailbox::new());
        let close_reason = Arc::new(Mutex::new(None));
        let reader =
            thread::spawn(move || run_reader_loop(stream, pending, frame_stats, close_reason));
        let result = rx.recv_timeout(Duration::from_secs(5));
        drop(rx);
        let _ = reader.join();
        result
    }

    #[test]
    fn a_response_split_across_the_read_timeout_boundary_still_resolves() {
        // Regression test for the fix in this module (review Major P2-M3):
        // the reader thread used to `raw.clear()` at the top of every loop
        // iteration, so a socket read timeout landing mid-line silently
        // threw away whatever had already been read for it. Verified to
        // fail against the pre-fix code.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let read_timeout = Duration::from_millis(100);
        let _server = thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let response = Response::success(
                7,
                serde_json::to_value(HandshakeInfo {
                    app_name: "fake-app".into(),
                    frust_version: "0.0.0".into(),
                    protocol_version: PROTOCOL_VERSION,
                    capabilities: vec![Capability::WidgetTree],
                })
                .unwrap(),
            );
            let full = format!("{}\n", encode_line(&response));
            let bytes = full.as_bytes();
            let split = bytes.len() / 2;

            let mut writer = &stream;
            writer.write_all(&bytes[..split]).unwrap();
            writer.flush().unwrap();
            // Well past `read_timeout`, so the reader thread's in-flight
            // `read()` is guaranteed to hit at least one real
            // `WouldBlock`/`TimedOut` tick with only the first half
            // buffered.
            thread::sleep(read_timeout * 3);
            writer.write_all(&bytes[split..]).unwrap();
            writer.flush().unwrap();
            // Hold the connection open a beat past that so the reader
            // thread's next read isn't racing this thread's exit/close.
            thread::sleep(Duration::from_millis(100));
        });

        let client_stream = TcpStream::connect(addr).unwrap();
        let start = Instant::now();
        let response = drive_reader_loop_for_one_response(client_stream, read_timeout, 7)
            .expect("a response split across a read-timeout tick must still resolve");
        assert!(
            start.elapsed() >= read_timeout,
            "the exchange should have spanned at least one read-timeout tick, took {:?}",
            start.elapsed()
        );
        let ResponseOutcome::Success { result } = response.outcome else {
            panic!("expected a success response, got {:?}", response.outcome);
        };
        let handshake: HandshakeInfo = serde_json::from_value(result).unwrap();
        assert_eq!(handshake.app_name, "fake-app");
        assert_eq!(handshake.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn a_response_split_mid_multibyte_utf8_across_the_timeout_boundary_still_resolves() {
        // The `read_until`-shaped byte-accurate reassembly (over
        // `read_line`'s string-append semantics) matters specifically here:
        // a chunk boundary landing inside a multibyte UTF-8 sequence must
        // not lose the bytes already read.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let read_timeout = Duration::from_millis(100);
        let _server = thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let response = Response::success(
                9,
                serde_json::to_value(WidgetTreeDump {
                    roots: vec![WidgetNode {
                        id: 1,
                        // "ö" encodes as the two continuation bytes 0xC3 0xB6
                        // — the split below lands between them.
                        type_name: "Röot".into(),
                        debug_label: None,
                        bounds: None,
                        children: vec![],
                    }],
                })
                .unwrap(),
            );
            let full = format!("{}\n", encode_line(&response));
            let bytes = full.as_bytes();
            let split = bytes
                .windows(2)
                .position(|w| w == [0xC3, 0xB6])
                .expect("the multibyte character must appear in the encoded line")
                + 1;

            let mut writer = &stream;
            writer.write_all(&bytes[..split]).unwrap();
            writer.flush().unwrap();
            thread::sleep(read_timeout * 3);
            writer.write_all(&bytes[split..]).unwrap();
            writer.flush().unwrap();
            thread::sleep(Duration::from_millis(100));
        });

        let client_stream = TcpStream::connect(addr).unwrap();
        let response = drive_reader_loop_for_one_response(client_stream, read_timeout, 9)
            .expect("a response split mid-multibyte-UTF-8 across a timeout tick must resolve");
        let ResponseOutcome::Success { result } = response.outcome else {
            panic!("expected a success response, got {:?}", response.outcome);
        };
        let tree: WidgetTreeDump = serde_json::from_value(result).unwrap();
        assert_eq!(tree.roots[0].type_name, "Röot");
    }

    #[test]
    fn an_oversized_line_is_refused_rather_than_buffered_unbounded() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            // A well-behaved peer never does this (`frust-devtools` enforces
            // the identical cap on its own read side); this stands in for a
            // hostile or badly wedged one. Never writes a newline, and
            // writes past `MAX_LINE_BYTES` so the client's cap must trip
            // before it would ever find one.
            let mut writer = &stream;
            let chunk = vec![b'x'; 64 * 1024];
            for _ in 0..(MAX_LINE_BYTES / chunk.len() + 2) {
                if writer.write_all(&chunk).is_err() {
                    break;
                }
            }
            // Hold the connection open so the client side isn't racing this
            // thread's exit/close while it's still consuming the flood.
            thread::sleep(Duration::from_millis(200));
        });

        let client_stream = TcpStream::connect(addr).unwrap();
        client_stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let reader_stream = client_stream.try_clone().unwrap();
        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::channel();
        pending.lock().unwrap().insert(1, tx);
        let frame_stats = Arc::new(FrameStatsMailbox::new());
        let close_reason = Arc::new(Mutex::new(None));
        let reader_close_reason = Arc::clone(&close_reason);
        let reader = thread::spawn(move || {
            run_reader_loop(reader_stream, pending, frame_stats, reader_close_reason)
        });

        let start = Instant::now();
        // The reader thread must give up on this connection rather than
        // buffer the peer's line forever: the pending sender is dropped, so
        // the receiving end disconnects instead of ever getting a
        // `Response`.
        let recv_err = rx.recv_timeout(Duration::from_secs(5)).unwrap_err();
        assert!(
            matches!(recv_err, mpsc::RecvTimeoutError::Disconnected),
            "expected the pending sender to be dropped once the line-length cap trips, got: {recv_err:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "an oversized line should be refused promptly rather than buffered indefinitely, took {:?}",
            start.elapsed()
        );

        let reason = close_reason
            .lock()
            .unwrap()
            .clone()
            .expect("the reader thread should record why it closed the connection");
        assert!(
            reason.contains(&MAX_LINE_BYTES.to_string()),
            "expected a clear oversized-line message naming the cap, got: {reason}"
        );

        drop(client_stream);
        let _ = reader.join();
    }

    #[test]
    fn discovery_parses_desktop_and_logcat_prefixed_lines_via_the_protocol_crate() {
        // Reuses `frust_devtools_protocol::parse_discovery_line` directly —
        // no local re-implementation of discovery-line parsing exists in
        // this crate.
        use frust_devtools_protocol::{Discovery, parse_discovery_line};
        assert_eq!(
            parse_discovery_line("frust-devtools listening on 54321").map(|d| d.port),
            Some(54321)
        );
        assert_eq!(
            parse_discovery_line(
                "08-10 12:00:00.123  1234  5678 I Frust   : frust-devtools listening on 8123"
            )
            .map(|d| d.port),
            Some(8123)
        );
        assert_eq!(parse_discovery_line("some unrelated log line"), None);

        // The token rides the same line, and is what `connect` is handed.
        assert_eq!(
            parse_discovery_line(
                "08-10 12:00:00.123  1234  5678 I Frust   : frust-devtools listening on 8123 \
                 token 0123456789abcdef0123456789abcdef"
            ),
            Some(Discovery {
                port: 8123,
                token: Some("0123456789abcdef0123456789abcdef".to_string()),
            })
        );
    }

    #[test]
    fn adb_forward_ephemeral_parses_allocated_port() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 forward tcp:0 tcp:9229",
            Output {
                success: true,
                stdout: "39217\n".into(),
                stderr: String::new(),
            },
        );
        let port = adb_forward_ephemeral(&runner, "emulator-5554", 9229).unwrap();
        assert_eq!(port, 39217);
    }

    #[test]
    fn adb_forward_ephemeral_errs_on_garbage_output() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 forward tcp:0 tcp:9229",
            Output {
                success: true,
                stdout: "not-a-port\n".into(),
                stderr: String::new(),
            },
        );
        assert!(adb_forward_ephemeral(&runner, "emulator-5554", 9229).is_err());
    }

    #[test]
    fn adb_forward_ephemeral_errs_on_failed_exit() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 forward tcp:0 tcp:9229",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error: no devices/emulators found".into(),
            },
        );
        assert!(adb_forward_ephemeral(&runner, "emulator-5554", 9229).is_err());
    }

    #[test]
    fn adb_forward_remove_succeeds() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 forward --remove tcp:39217",
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        adb_forward_remove(&runner, "emulator-5554", 39217).unwrap();
    }

    #[test]
    fn adb_forward_remove_errs_on_failed_exit() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 forward --remove tcp:39217",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error: forward not found".into(),
            },
        );
        assert!(adb_forward_remove(&runner, "emulator-5554", 39217).is_err());
    }

    fn hot_script() -> Script {
        Script {
            capabilities: vec![Capability::WidgetTree, Capability::HotPatch],
            info: Some(HotpatchInfo {
                anchor_runtime: 0x1_0000_4000,
                pid: 4242,
                triple: "aarch64-apple-darwin".into(),
                patches_applied: 0,
                patch_bytes_loaded: 0,
                pending_layout_mismatches: Vec::new(),
            }),
            ..Script::default()
        }
    }

    fn outcome(applied: bool, patches_applied: u32) -> PatchOutcome {
        PatchOutcome {
            applied,
            seam_hits: 2,
            seam_fall_throughs: vec![MissedKey {
                image: 1,
                link_address: 0x40,
            }],
            layout_mismatches: Vec::new(),
            patches_applied,
            patch_bytes_loaded: 3,
        }
    }

    fn apply_params(patch_id: u64, len: u64) -> ApplyPatchParams {
        ApplyPatchParams {
            patch_id,
            len,
            pid: 4242,
            anchor_runtime: 0x1_0000_4000,
            table: JumpTableWire {
                map: [(0x10, 0x20)].into_iter().collect(),
                aslr_reference: 0x4000,
                new_base_address: 0x8000,
                ifunc_count: 0,
            },
            expected_seams: 1,
        }
    }

    #[test]
    fn base64_matches_the_rfc_4648_test_vectors() {
        for (raw, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode_base64(raw.as_bytes()), encoded);
            assert_eq!(test_server::decode_base64(encoded).unwrap(), raw.as_bytes());
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(
            test_server::decode_base64(&encode_base64(&all)).unwrap(),
            all
        );
    }

    #[test]
    fn patch_chunks_are_ordered_and_capped_at_512_kib_raw() {
        let bytes: Vec<u8> = (0..(PATCH_CHUNK_MAX_BYTES * 2 + 7))
            .map(|i| (i % 251) as u8)
            .collect();
        let chunks = patch_chunks(9, &bytes);
        assert_eq!(chunks.len(), 3);
        let mut reassembled = Vec::new();
        for (index, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.patch_id, 9);
            assert_eq!(chunk.total_len, bytes.len() as u64);
            assert_eq!(chunk.offset, (index * PATCH_CHUNK_MAX_BYTES) as u64);
            let raw = test_server::decode_base64(&chunk.data_base64).unwrap();
            assert!(raw.len() <= PATCH_CHUNK_MAX_BYTES);
            reassembled.extend(raw);
        }
        assert_eq!(reassembled, bytes);
        assert_eq!(chunks[2].data_base64.len(), encode_base64(&[0; 7]).len());
    }

    #[test]
    fn hotpatch_info_upload_and_apply_round_trip_against_a_capable_server() {
        let mut script = hot_script();
        script
            .pending
            .push_back(vec!["HomeState changed layout".into()]);
        script
            .applies
            .push_back(ApplyReply::Outcome(outcome(true, 1)));
        let server = test_server::spawn(script);
        let client =
            DevtoolsClient::connect(server.addr, Duration::from_secs(2), Some(FAKE_TOKEN)).unwrap();
        client.handshake().unwrap();

        let info = client.hotpatch_info().unwrap();
        assert_eq!(info.anchor_runtime, 0x1_0000_4000);
        assert_eq!(info.pid, 4242);
        assert_eq!(
            info.pending_layout_mismatches,
            vec!["HomeState changed layout".to_string()]
        );

        let bytes: Vec<u8> = (0..(PATCH_CHUNK_MAX_BYTES + 100))
            .map(|i| (i % 7) as u8)
            .collect();
        client.upload_patch(5, &bytes).unwrap();
        assert_eq!(server.uploaded(5), Some(bytes.clone()));

        let result = client
            .apply_patch(&apply_params(5, bytes.len() as u64))
            .unwrap();
        assert_eq!(result, outcome(true, 1));
        assert_eq!(
            server.methods(),
            vec![
                "handshake",
                "hotpatch_info",
                "patch_chunk",
                "patch_chunk",
                "apply_patch"
            ]
        );
        assert_eq!(client.hotpatch_info().unwrap().patches_applied, 1);
        assert!(client.peer_addr().unwrap().ip().is_loopback());
    }

    #[test]
    fn hot_patch_methods_without_the_capability_are_typed_rejections() {
        let server = test_server::spawn(Script {
            capabilities: vec![Capability::WidgetTree],
            unsupported_reason: "hot patching is off: the devtools token is not OS-sourced".into(),
            ..Script::default()
        });
        let client =
            DevtoolsClient::connect(server.addr, Duration::from_secs(2), Some(FAKE_TOKEN)).unwrap();
        client.handshake().unwrap();

        let err = client.hotpatch_info().unwrap_err();
        assert!(is_not_supported(&err), "{err}");
        assert!(format!("{err}").contains("not OS-sourced"), "{err}");

        let err = client.upload_patch(1, b"abc").unwrap_err();
        assert!(is_method_not_found(&err), "{err}");
        let err = client.apply_patch(&apply_params(1, 3)).unwrap_err();
        assert!(is_method_not_found(&err), "{err}");
        assert!(!is_not_supported(&err));
    }

    #[test]
    fn apply_patch_outlives_a_short_connection_timeout() {
        let mut script = hot_script();
        script.applies.push_back(ApplyReply::Delayed(
            Duration::from_millis(600),
            outcome(true, 1),
        ));
        let server = test_server::spawn(script);
        let client =
            DevtoolsClient::connect(server.addr, Duration::from_millis(200), Some(FAKE_TOKEN))
                .unwrap();
        client.handshake().unwrap();
        client.upload_patch(1, b"abc").unwrap();
        assert_eq!(
            client.apply_patch(&apply_params(1, 3)).unwrap(),
            outcome(true, 1)
        );
    }

    #[test]
    fn a_lost_apply_patch_reply_is_an_error_but_not_an_rpc_rejection() {
        let mut script = hot_script();
        script.applies.push_back(ApplyReply::Hangup);
        let server = test_server::spawn(script);
        let client =
            DevtoolsClient::connect(server.addr, Duration::from_secs(2), Some(FAKE_TOKEN)).unwrap();
        client.handshake().unwrap();
        client.upload_patch(1, b"abc").unwrap();
        let start = Instant::now();
        let err = client.apply_patch(&apply_params(1, 3)).unwrap_err();
        assert!(
            err.downcast_ref::<DevtoolsRpcError>().is_none(),
            "a closed connection leaves the outcome unknown, not refused: {err}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }
}
