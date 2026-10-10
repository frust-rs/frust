//! The accept loop and per-connection task: sockets, NDJSON framing, and
//! shutdown. Every protocol decision lives in [`crate::dispatch`].

use std::sync::Arc;
use std::time::Duration;

use frust_devtools_protocol::encode_line;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, broadcast, watch};
use tokio::task::JoinSet;

use crate::dispatch::{self, ConnState, Decoded, SessionCtx, SideEffect};

/// A single request line longer than this is treated as a broken client: it
/// is answered with an error naming the cap
/// ([`dispatch::oversized_line_response`]) and the connection closes. Nothing
/// in the v1 protocol needs a longer line (a patch and its jump table travel
/// as chunks under 1 MiB each); the cap exists so a peer that never sends a
/// newline cannot grow this process's memory without bound.
const MAX_LINE_BYTES: usize = 1 << 20;

/// How long a connection that sent an over-long line keeps reading (and
/// discarding) what its client still sends after the error reply, before it
/// closes. Closing with unread input pending would reset the connection and
/// could destroy the reply in flight; draining lets the client read it.
const OVERSIZED_DRAIN: Duration = Duration::from_secs(2);

/// Read chunk size for the line reader — one page.
const READ_CHUNK_BYTES: usize = 4096;

/// How long [`accept_loop`] lets in-flight connections finish their current
/// write after shutdown is signalled, before aborting them. A connection only
/// ever owes a client one line at this point, so this is generous.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(250);

/// Accepts and serves connections until `shutdown` flips to `true`.
///
/// Concurrency is capped by `max_clients`: past that, a new connection is
/// closed immediately rather than queued, so a port scanner or a runaway
/// client cannot spawn unbounded tasks.
pub(crate) async fn accept_loop(
    listener: TcpListener,
    ctx: Arc<SessionCtx>,
    mut shutdown: watch::Receiver<bool>,
    max_clients: usize,
) {
    let permits = Arc::new(Semaphore::new(max_clients.max(1)));
    let mut connections = JoinSet::new();

    if *shutdown.borrow() {
        return;
    }

    loop {
        tokio::select! {
            // Shutdown wins over a ready accept: a service being torn down
            // should not pick up one more client on the way out.
            biased;

            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }

            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                        log::warn!(
                            "frust-devtools: at the {max_clients}-client cap, refusing {peer}"
                        );
                        drop(stream);
                        continue;
                    };
                    log::debug!("frust-devtools: client connected from {peer}");
                    let conn_ctx = Arc::clone(&ctx);
                    let conn_shutdown = shutdown.clone();
                    connections.spawn(async move {
                        connection(stream, conn_ctx, conn_shutdown).await;
                        log::debug!("frust-devtools: client {peer} disconnected");
                        drop(permit);
                    });
                }
                Err(e) => {
                    // A per-connection accept failure (a client that vanished
                    // between SYN and accept, a momentary fd shortage) is not
                    // fatal to the listener; a persistently broken listener
                    // just logs each time rather than spinning silently.
                    log::warn!("frust-devtools: accept failed: {e}");
                }
            },
        }

        // Reap finished connection tasks without waiting on any of them —
        // otherwise the JoinSet grows for the process lifetime.
        while connections.try_join_next().is_some() {}
    }

    // Graceful close: connections observe the same `shutdown` watch and exit
    // after their current write, so give them a bounded window before the
    // abort that guarantees this returns.
    let drain = async { while connections.join_next().await.is_some() {} };
    if tokio::time::timeout(SHUTDOWN_GRACE, drain).await.is_err() {
        log::warn!("frust-devtools: a client did not close within the grace window, aborting it");
    }
    connections.shutdown().await;
}

/// Serves one client until it disconnects or the service shuts down.
///
/// Reads and writes share one task on purpose: the two never need to overlap
/// (a response and a notification are both just a line to write), and a single
/// task makes the ordering obvious — a `frame_stats` push can never interleave
/// mid-response. The cost is that a client which stops reading eventually
/// stalls *its own* connection in `write_all`; that connection is dropped at
/// shutdown, and it affects nobody else.
async fn connection(stream: TcpStream, ctx: Arc<SessionCtx>, mut shutdown: watch::Receiver<bool>) {
    // Debug traffic is small and latency-sensitive: send each line as it is
    // produced rather than coalescing.
    if let Err(e) = stream.set_nodelay(true) {
        log::debug!("frust-devtools: could not set TCP_NODELAY: {e}");
    }
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = LineReader::new(read_half);
    let mut stats: Option<broadcast::Receiver<frust_devtools_protocol::FrameStats>> = None;
    // Per-connection, never shared: one client's handshake must not authorize
    // any other socket (`crate::dispatch`'s `ConnState`).
    let mut conn = ConnState::new(&ctx);

    if *shutdown.borrow() {
        return;
    }

    loop {
        let step = tokio::select! {
            biased;

            _ = shutdown.changed() => Step::Shutdown,

            frame = next_frame_stats(&mut stats) => frame,

            line = reader.next_line() => match line {
                Ok(Some(line)) => Step::Line(line),
                Ok(None) => Step::Eof,
                Err(ReadError::Oversized { id }) => Step::Oversized(id),
                Err(ReadError::Io(e)) => {
                    log::debug!("frust-devtools: read error, closing connection: {e}");
                    Step::Eof
                }
            },
        };

        match step {
            Step::Shutdown | Step::Eof => break,

            Step::Oversized(id) => {
                log::warn!(
                    "frust-devtools: a request line exceeded {MAX_LINE_BYTES} bytes; answering \
                     and closing the connection"
                );
                let line = encode_line(&dispatch::oversized_line_response(id, MAX_LINE_BYTES));
                if write_line(&mut write_half, &line).await.is_ok() {
                    let _ = write_half.shutdown().await;
                    let _ = tokio::time::timeout(OVERSIZED_DRAIN, reader.discard_to_eof()).await;
                }
                break;
            }

            Step::Stats(frame) => {
                let line = encode_line(&dispatch::frame_stats_notification(frame));
                if write_line(&mut write_half, &line).await.is_err() {
                    break;
                }
            }

            Step::StatsLagged(missed) => {
                // Bounded, deliberate loss — the client is slower than the
                // frame thread. `docs/CODE_STANDARDS.md`'s frame-gate rules
                // forbid making the producer wait for it.
                log::debug!("frust-devtools: client missed {missed} frame-stats pushes");
            }

            Step::StatsClosed => stats = None,

            Step::Line(line) => {
                let response = match dispatch::decode(&line) {
                    Decoded::Request(req) => {
                        let (response, effect) =
                            dispatch::handle_request(&ctx, &mut conn, &req).await;
                        if effect == SideEffect::SubscribeFrameStats && stats.is_none() {
                            stats = Some(ctx.bus.subscribe());
                        }
                        Some(response)
                    }
                    Decoded::Ignore(why) => {
                        log::debug!("frust-devtools: ignoring a client line — {why}");
                        None
                    }
                    Decoded::Malformed { id, detail } => {
                        Some(dispatch::parse_error_response(id, &detail))
                    }
                    Decoded::Undeliverable(detail) => {
                        // No id: there is no response the client could route,
                        // so the connection simply stays open for the next
                        // line (resilience, not silence — it is logged).
                        log::warn!("frust-devtools: skipping an undecodable line — {detail}");
                        None
                    }
                };

                if let Some(response) = response {
                    let line = encode_line(&response);
                    if write_line(&mut write_half, &line).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

/// One iteration's outcome in [`connection`]'s loop. Named rather than handled
/// inline so no `select!` branch body holds a borrow another branch needs.
enum Step {
    Shutdown,
    Eof,
    Line(String),
    /// A line grew past [`MAX_LINE_BYTES`]; the `id` salvaged from its start.
    Oversized(Option<u64>),
    Stats(frust_devtools_protocol::FrameStats),
    StatsLagged(u64),
    StatsClosed,
}

/// Awaits the next frame-stats push, or never resolves while this client has
/// not subscribed. Cancel-safe (`broadcast::Receiver::recv` is), which is what
/// lets it sit in a `select!` branch.
async fn next_frame_stats(
    stats: &mut Option<broadcast::Receiver<frust_devtools_protocol::FrameStats>>,
) -> Step {
    match stats {
        None => std::future::pending().await,
        Some(rx) => match rx.recv().await {
            Ok(frame) => Step::Stats(frame),
            Err(broadcast::error::RecvError::Lagged(missed)) => Step::StatsLagged(missed),
            Err(broadcast::error::RecvError::Closed) => Step::StatsClosed,
        },
    }
}

async fn write_line(
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    line: &str,
) -> std::io::Result<()> {
    write_half.write_all(line.as_bytes()).await?;
    write_half.write_all(b"\n").await
}

/// Why [`LineReader::next_line`] produced no line.
#[derive(Debug)]
enum ReadError {
    /// The socket failed.
    Io(std::io::Error),
    /// The buffered line passed [`MAX_LINE_BYTES`] without a newline; `id` is
    /// the request id [`dispatch::salvage_id`] read off its start, if any.
    Oversized { id: Option<u64> },
}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        ReadError::Io(e)
    }
}

/// NDJSON line reader.
///
/// Hand-rolled over [`AsyncReadExt::read`] rather than
/// `tokio::io::BufReader::lines` for two reasons: `read` is documented
/// cancel-safe (nothing is consumed when a `select!` branch loses), and the
/// [`MAX_LINE_BYTES`] cap needs to be enforced *while* buffering, not after.
struct LineReader<R> {
    inner: R,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            buf: Vec::with_capacity(READ_CHUNK_BYTES),
        }
    }

    /// Next complete line (terminator stripped), or `None` at EOF.
    ///
    /// Cancel-safe: buffered bytes stay in `self.buf`, and the awaited `read`
    /// consumes nothing when it is dropped.
    async fn next_line(&mut self) -> Result<Option<String>, ReadError> {
        loop {
            if let Some(newline) = self.buf.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=newline).collect();
                line.pop(); // the '\n'
                if line.last() == Some(&b'\r') {
                    line.pop(); // tolerate CRLF from a hand-driven client
                }
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
            if self.buf.len() > MAX_LINE_BYTES {
                let id = dispatch::salvage_id(&self.buf);
                self.buf.clear();
                return Err(ReadError::Oversized { id });
            }

            let mut chunk = [0u8; READ_CHUNK_BYTES];
            let read = self.inner.read(&mut chunk).await?;
            if read == 0 {
                // A trailing partial line is not a request — an unterminated
                // line is by definition incomplete under NDJSON framing.
                return Ok(None);
            }
            self.buf.extend_from_slice(&chunk[..read]);
        }
    }

    /// Reads and discards everything until the peer closes or the socket
    /// fails: what is left of an over-long line, after its error reply.
    async fn discard_to_eof(&mut self) {
        self.buf.clear();
        let mut chunk = [0u8; READ_CHUNK_BYTES];
        while let Ok(read) = self.inner.read(&mut chunk).await {
            if read == 0 {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives [`LineReader`] over an in-memory reader — no socket, no runtime
    /// beyond the tiny one this test builds.
    fn read_all_lines(input: &[u8]) -> Result<Vec<String>, ReadError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("current-thread runtime");
        rt.block_on(async {
            let mut reader = LineReader::new(input);
            let mut lines = Vec::new();
            while let Some(line) = reader.next_line().await? {
                lines.push(line);
            }
            Ok(lines)
        })
    }

    #[test]
    fn splits_on_newlines_and_drops_the_terminator() {
        let lines = read_all_lines(b"one\ntwo\nthree\n").unwrap();
        assert_eq!(lines, vec!["one", "two", "three"]);
    }

    #[test]
    fn tolerates_crlf() {
        let lines = read_all_lines(b"one\r\ntwo\r\n").unwrap();
        assert_eq!(lines, vec!["one", "two"]);
    }

    #[test]
    fn an_unterminated_trailing_line_is_not_a_line() {
        let lines = read_all_lines(b"one\npartial").unwrap();
        assert_eq!(lines, vec!["one"]);
    }

    #[test]
    fn a_line_spanning_many_read_chunks_is_reassembled() {
        let mut input = vec![b'x'; READ_CHUNK_BYTES * 3];
        input.push(b'\n');
        let lines = read_all_lines(&input).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), READ_CHUNK_BYTES * 3);
    }

    #[test]
    fn an_unbounded_line_is_refused_rather_than_buffered_forever() {
        let input = vec![b'x'; MAX_LINE_BYTES + READ_CHUNK_BYTES];
        let err = read_all_lines(&input).unwrap_err();
        assert!(matches!(err, ReadError::Oversized { id: None }), "{err:?}");
    }

    #[test]
    fn an_over_long_request_line_keeps_the_id_it_started_with() {
        let mut input =
            br#"{"jsonrpc":"2.0","id":42,"method":"input_text","params":{"text":""#.to_vec();
        input.resize(MAX_LINE_BYTES + READ_CHUNK_BYTES, b'x');
        let err = read_all_lines(&input).unwrap_err();
        assert!(
            matches!(err, ReadError::Oversized { id: Some(42) }),
            "{err:?}"
        );
    }

    /// The smallest backend there is: nothing here is reached by a line the
    /// reader refuses.
    struct Inert;

    impl crate::DevtoolsBackend for Inert {
        fn widget_tree(&self) -> frust_devtools_protocol::WidgetTreeDump {
            frust_devtools_protocol::WidgetTreeDump { roots: Vec::new() }
        }
        fn widget_props(&self, _id: u64) -> Option<frust_devtools_protocol::WidgetProps> {
            None
        }
        fn metrics_snapshot(&self) -> frust_devtools_protocol::MetricsSnapshot {
            frust_devtools_protocol::MetricsSnapshot {
                rss_bytes: None,
                uptime_ms: 0,
            }
        }
        fn inject_tap(
            &self,
            _p: frust_devtools_protocol::InputTapParams,
        ) -> Result<(), crate::BackendError> {
            Ok(())
        }
        fn inject_scroll(
            &self,
            _p: frust_devtools_protocol::InputScrollParams,
        ) -> Result<(), crate::BackendError> {
            Ok(())
        }
        fn inject_text(&self, _t: &str) -> Result<(), crate::BackendError> {
            Ok(())
        }
    }

    #[test]
    fn an_over_long_line_is_answered_with_an_error_naming_the_cap_before_the_close() {
        use std::io::{BufRead as _, Write as _};

        let service = crate::Service::start_with_config(
            Inert,
            crate::AppInfo::new("over-long", "0.0.0"),
            crate::ServiceConfig {
                require_token: false,
                ..crate::ServiceConfig::default()
            },
        )
        .expect("service starts");
        let stream = std::net::TcpStream::connect(("127.0.0.1", service.port())).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let mut writer = stream.try_clone().expect("clone");
        // Written on its own thread: the server stops buffering at the cap,
        // so the write completes only through its drain.
        let sender = std::thread::spawn(move || {
            let mut line =
                br#"{"jsonrpc":"2.0","id":42,"method":"input_text","params":{"text":""#.to_vec();
            line.resize(MAX_LINE_BYTES + 64 * 1024, b'x');
            line.extend_from_slice(b"\"}}\n");
            let _ = writer.write_all(&line);
        });

        let mut reader = std::io::BufReader::new(stream);
        let mut reply = String::new();
        reader
            .read_line(&mut reply)
            .expect("an error reply arrives");
        let response: frust_devtools_protocol::Response =
            frust_devtools_protocol::serde_json::from_str(reply.trim_end()).expect("a response");
        assert_eq!(response.id, 42);
        match response.outcome {
            frust_devtools_protocol::ResponseOutcome::Error { error } => {
                assert_eq!(
                    error.code,
                    frust_devtools_protocol::RpcError::INVALID_REQUEST
                );
                assert!(
                    error.message.contains(&MAX_LINE_BYTES.to_string()),
                    "{}",
                    error.message
                );
            }
            other => panic!("expected an error, got {other:?}"),
        }
        // ...and then the connection closes.
        let mut rest = String::new();
        assert_eq!(reader.read_line(&mut rest).expect("eof"), 0, "{rest}");
        sender.join().expect("sender");
        service.shutdown();
    }

    #[test]
    fn empty_lines_are_produced_not_swallowed() {
        // Dispatch classifies an empty line as undecodable and skips it; the
        // reader itself must not silently merge it into the next one.
        let lines = read_all_lines(b"\nok\n").unwrap();
        assert_eq!(lines, vec!["", "ok"]);
    }
}
