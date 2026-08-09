//! The accept loop and per-connection task: sockets, NDJSON framing, and
//! shutdown. Every protocol decision lives in [`crate::dispatch`].

use std::sync::Arc;
use std::time::Duration;

use frust_devtools_protocol::encode_line;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, broadcast, watch};
use tokio::task::JoinSet;

use crate::dispatch::{self, Decoded, SessionCtx, SideEffect};

/// A single request line longer than this is treated as a broken client and
/// closes the connection. Nothing in the v1 protocol comes close (the largest
/// client→server payload is an `input_text` string); the cap exists so a
/// peer that never sends a newline cannot grow this process's memory without
/// bound.
const MAX_LINE_BYTES: usize = 1 << 20;

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
                Err(e) => {
                    log::debug!("frust-devtools: read error, closing connection: {e}");
                    Step::Eof
                }
            },
        };

        match step {
            Step::Shutdown | Step::Eof => break,

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
                        let (response, effect) = dispatch::handle_request(&ctx, &req).await;
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
    async fn next_line(&mut self) -> std::io::Result<Option<String>> {
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
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("devtools line exceeded {MAX_LINE_BYTES} bytes without a newline"),
                ));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives [`LineReader`] over an in-memory reader — no socket, no runtime
    /// beyond the tiny one this test builds.
    fn read_all_lines(input: &[u8]) -> std::io::Result<Vec<String>> {
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
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn empty_lines_are_produced_not_swallowed() {
        // Dispatch classifies an empty line as undecodable and skips it; the
        // reader itself must not silently merge it into the next one.
        let lines = read_all_lines(b"\nok\n").unwrap();
        assert_eq!(lines, vec!["", "ok"]);
    }
}
