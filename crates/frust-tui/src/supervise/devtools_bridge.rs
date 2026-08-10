//! The impure half of DevTools mode (workbook §B12): one thread per session
//! owning a [`DevtoolsClient`] connection, reporting everything it learns back
//! into the engine's message channel as [`ConnEvent`]s.
//!
//! # Why a thread, not the event loop
//!
//! [`DevtoolsClient`] is blocking by charter (`docs/DEVTOOLS_ARCHITECTURE.md`
//! — `std::net::TcpStream`, no tokio), and the connect + `handshake` +
//! `adb forward` sequence can take seconds against a device. None of it may
//! happen while `crate::runner`'s `select!` loop is enacting an effect, so
//! [`DevtoolsBridge::connect`] only *spawns*: the socket work, the whole
//! failure vocabulary, and the frame-stats pump all live on the spawned
//! thread — exactly the split `crate::supervise::supervisor` makes for a
//! session's blocking line drain.
//!
//! # Coalescing (the same shape the session drain uses)
//!
//! A running app publishes frame stats at its frame rate. Forwarding one
//! engine message per frame would spend the whole event loop on redraw
//! bookkeeping, so the pump batches: it collects samples for
//! [`COALESCE_WINDOW`] and forwards at most one
//! [`ConnEvent::Frames`] batch per window — ≤ ~30 messages a second
//! regardless of the app's frame rate — capped at [`MAX_BATCH_FRAMES`]
//! samples, dropping the *oldest* beyond that. Two bounded hops already sit
//! upstream of this one (the service's broadcast bus, then
//! `DevtoolsClient`'s single-slot drop-oldest mailbox), and the engine's ring
//! ([`crate::engine::FRAME_RING_CAP`]) bounds what is retained downstream, so
//! a slow consumer only ever loses the oldest samples and never grows memory.
//!
//! # Android
//!
//! The service binds device loopback, so an Android session first allocates a
//! host port with `adb forward` ([`adb_forward_ephemeral`]) and connects to
//! that; the forward is removed on the way out
//! ([`adb_forward_remove`]), whether the connection ended cleanly or not.
//! Desktop (and the iOS simulator) connect straight to `127.0.0.1:port`.
//!
//! # Lifetime
//!
//! A connection lives until it is explicitly disconnected (session end, or a
//! reconnect replacing it — see `crate::engine::update`'s retention policy),
//! the peer goes away (the client's frame-stats relay closes, surfaced as
//! [`ConnEvent::Failed`]), or the engine drops its receiver. [`Drop`] stops
//! and joins every thread, so a torn-down bridge leaks neither a thread nor
//! an `adb` forward.
//!
//! Disconnecting *joins* rather than detaching, so a replaced connection can
//! never race a stale thread's late report into the engine for the same
//! session. The cost is that a disconnect waits for the thread's current
//! step: at most one [`COALESCE_WINDOW`] while pumping, or
//! [`CONNECT_TIMEOUT`] while still trying to reach a service that never
//! answers — which is why the reachability probe is explicitly bounded
//! rather than left to the platform's default connect behavior.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use frust_devtools_protocol::FrameStats;
use frust_drive::devtools_client::{DevtoolsClient, adb_forward_ephemeral, adb_forward_remove};
use frust_drive::process::ProcessRunner;
use tokio::sync::mpsc::UnboundedSender;

use crate::engine::{ConnEvent, DevtoolsTarget, Message};

use super::session::SessionId;

/// Socket read/write timeout, and the bound each request waits for its
/// response. Generous enough for a device round trip over `adb forward`,
/// short enough that a wrong port surfaces as §B12's failed screen rather
/// than an indefinite spinner.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Bound on the initial TCP reachability probe (see [`connect_and_pump`]) —
/// and therefore on the longest a `disconnect` can wait for a bridge thread
/// that is still trying to reach a service that never answers. §B12's failed
/// screen says "refused after 3s" for exactly this bound.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the pump gathers frame-stats samples before forwarding them as
/// one batch — the coalescing window that caps engine traffic at ~30
/// messages/second, and also the interval at which the pump notices a
/// disconnect request.
const COALESCE_WINDOW: Duration = Duration::from_millis(33);

/// Cap on one forwarded batch. A window this short can only fill it if the
/// app is publishing absurdly fast; past the cap the *oldest* samples in the
/// batch are dropped, matching every other hop on this path.
const MAX_BATCH_FRAMES: usize = 120;

/// Owns one devtools connection thread per session and the stop flags that
/// tear them down.
pub struct DevtoolsBridge {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    conns: HashMap<SessionId, Conn>,
}

/// One live (or finishing) bridge thread.
struct Conn {
    /// Set to ask the thread to wind down at its next window tick.
    stop: Arc<AtomicBool>,
    /// Joined on disconnect/drop so no thread — and no `adb` forward —
    /// outlives the bridge.
    worker: Option<JoinHandle<()>>,
}

impl DevtoolsBridge {
    /// Build a bridge over `runner` (the same [`ProcessRunner`] seam the
    /// supervisor uses — the `adb forward` calls go through it, so a test can
    /// script them).
    pub fn new(runner: Arc<dyn ProcessRunner + Send + Sync>) -> Self {
        Self {
            runner,
            conns: HashMap::new(),
        }
    }

    /// Open a connection for `target.session`, replacing any this bridge
    /// already holds for it (an app that restarted announces a fresh
    /// port/token — the old socket points at a dead port). Returns
    /// immediately; everything else happens on the spawned thread and comes
    /// back as [`Message::DevtoolsConn`].
    pub fn connect(&mut self, target: DevtoolsTarget, tx: UnboundedSender<Message>) {
        let session = target.session;
        self.disconnect(session);

        let stop = Arc::new(AtomicBool::new(false));
        let runner = Arc::clone(&self.runner);
        let worker = {
            let stop = Arc::clone(&stop);
            let tx = tx.clone();
            thread::Builder::new()
                .name(format!("frust-tui-devtools-{}", session.0))
                .spawn(move || run_bridge(target, runner, stop, tx))
        };
        match worker {
            Ok(worker) => {
                self.conns.insert(
                    session,
                    Conn {
                        stop,
                        worker: Some(worker),
                    },
                );
            }
            Err(err) => {
                // Spawning failed (a resource limit) — report it the same way
                // a refused connection reports, so the UI shows §B12's failed
                // screen with a real reason instead of spinning forever.
                let _ = tx.send(Message::DevtoolsConn(
                    session,
                    ConnEvent::Failed(format!("could not start the devtools bridge thread: {err}")),
                ));
            }
        }
    }

    /// Tear down `session`'s connection, if any: flag the thread, join it
    /// (it removes its own `adb forward` on the way out), and forget it.
    /// Idempotent; an unknown session is ignored.
    pub fn disconnect(&mut self, session: SessionId) {
        if let Some(mut conn) = self.conns.remove(&session) {
            conn.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = conn.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// Tear down every connection (the bridge's job on quit).
    pub fn disconnect_all(&mut self) {
        let sessions: Vec<SessionId> = self.conns.keys().copied().collect();
        for session in sessions {
            self.disconnect(session);
        }
    }
}

impl Drop for DevtoolsBridge {
    fn drop(&mut self) {
        self.disconnect_all();
    }
}

/// An allocated `adb forward`, remembered so it can be removed again.
struct Forward {
    serial: String,
    local_port: u16,
}

/// One bridge thread: allocate the Android forward (if any), run the
/// connection, then always undo the forward and report how it ended.
fn run_bridge(
    target: DevtoolsTarget,
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    stop: Arc<AtomicBool>,
    tx: UnboundedSender<Message>,
) {
    let session = target.session;
    let forward = match target.android_serial.as_deref() {
        Some(serial) => match adb_forward_ephemeral(runner.as_ref(), serial, target.port) {
            Ok(local_port) => Some(Forward {
                serial: serial.to_string(),
                local_port,
            }),
            Err(err) => {
                report(&tx, session, ConnEvent::Failed(format!("{err:#}")));
                return;
            }
        },
        None => None,
    };
    let local_port = forward
        .as_ref()
        .map(|f| f.local_port)
        .unwrap_or(target.port);

    let addr = SocketAddr::from(([127, 0, 0, 1], local_port));
    let result = connect_and_pump(addr, target.token.as_deref(), session, &stop, &tx);

    if let Some(forward) = forward {
        // Best-effort: the connection is over either way, and a failure to
        // remove the forward must not mask why it ended.
        let _ = adb_forward_remove(runner.as_ref(), &forward.serial, forward.local_port);
    }

    let event = match result {
        Ok(()) => ConnEvent::Closed,
        Err(err) => ConnEvent::Failed(format!("{err:#}")),
    };
    report(&tx, session, event);
}

/// Connect, handshake, arm the frame-stats subscription, and pump batches
/// until asked to stop (`Ok`) or the connection/engine goes away (`Err` /
/// `Ok` respectively — see [`pump_frames`]).
fn connect_and_pump(
    addr: SocketAddr,
    token: Option<&str>,
    session: SessionId,
    stop: &AtomicBool,
    tx: &UnboundedSender<Message>,
) -> Result<()> {
    // A bounded reachability probe first. `DevtoolsClient::connect` uses the
    // platform's own TCP connect behavior, which has no bound this side can
    // set — and `DevtoolsBridge::disconnect` *joins* this thread from the
    // event loop, so an unbounded connect here would be an unbounded stall
    // there. Probing with an explicit timeout puts the only wait that can
    // actually hang under [`CONNECT_TIMEOUT`]; the client's own connect that
    // follows is then to an already-proven-reachable loopback port.
    let probe = std::net::TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .with_context(|| format!("connecting to the devtools service at {addr}"))?;
    drop(probe);
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }
    let client = DevtoolsClient::connect(addr, REQUEST_TIMEOUT, token)
        .with_context(|| format!("connecting to the devtools service at {addr}"))?;
    let info = client
        .handshake()
        .context("the devtools handshake was rejected")?;
    if !report(
        tx,
        session,
        ConnEvent::Connected {
            app_name: info.app_name,
            caps: info.capabilities,
        },
    ) {
        return Ok(());
    }
    let frames = client
        .subscribe_frame_stats()
        .context("subscribing to devtools frame stats")?;
    pump_frames(&frames, session, stop, tx)
}

/// Forward coalesced frame-stats batches until `stop` is set (`Ok`), the
/// engine drops its receiver (`Ok` — nothing left to report to), or the
/// connection closes (`Err`, which surfaces §B12's failed screen).
fn pump_frames(
    frames: &mpsc::Receiver<FrameStats>,
    session: SessionId,
    stop: &AtomicBool,
    tx: &UnboundedSender<Message>,
) -> Result<()> {
    let mut batch: Vec<FrameStats> = Vec::new();
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        match frames.recv_timeout(COALESCE_WINDOW) {
            Ok(stats) => {
                batch.push(stats);
                if batch.len() > MAX_BATCH_FRAMES {
                    batch.remove(0);
                }
            }
            // The window elapsed: forward whatever accumulated. This is the
            // ordinary tick — an idle app simply forwards nothing.
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !flush(&mut batch, session, tx) {
                    return Ok(());
                }
            }
            // The client's reader thread closed the frame-stats mailbox: the
            // peer is gone.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                flush(&mut batch, session, tx);
                anyhow::bail!("the devtools connection closed");
            }
        }
    }
}

/// Forward `batch` (if non-empty) and clear it. Returns whether the engine is
/// still listening.
fn flush(batch: &mut Vec<FrameStats>, session: SessionId, tx: &UnboundedSender<Message>) -> bool {
    if batch.is_empty() {
        return !tx.is_closed();
    }
    report(tx, session, ConnEvent::Frames(std::mem::take(batch)))
}

/// Post one report into the engine channel. Returns whether it was delivered
/// — a closed channel means the engine is gone and the caller should wind
/// down.
fn report(tx: &UnboundedSender<Message>, session: SessionId, event: ConnEvent) -> bool {
    tx.send(Message::DevtoolsConn(session, event)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_devtools_protocol::{
        Capability, HandshakeInfo, Incoming, Method, Notification, PROTOCOL_VERSION, Response,
        RpcError, decode_line, encode_line, serde_json,
    };
    use frust_drive::process::FakeProcessRunner;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Instant;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    /// The token the canned server below requires — the stand-in for one
    /// recovered from a discovery line.
    const TOKEN: &str = "0123456789abcdef";

    /// A generous ceiling so a genuinely wedged test fails instead of hanging
    /// the suite, without flaking under a loaded `cargo test --workspace`.
    const RECV_TIMEOUT: Duration = Duration::from_secs(30);

    fn frame(n: u64) -> FrameStats {
        FrameStats {
            n,
            total_us: 16_000,
            rebuild_us: 8_000,
            layout_us: 3_000,
            paint_us: 2_000,
            encode_us: 1_400,
            acquire_us: 600,
            submit_us: 1_000,
            skipped: false,
        }
    }

    /// A hand-rolled NDJSON devtools server: accepts one connection, answers
    /// `handshake` (rejecting a wrong/missing token exactly as the real
    /// service does) and `frame_stats_subscribe`, then pushes `frame_count`
    /// `frame_stats` notifications. Deliberately *not* `frust-devtools` — the
    /// tooling charter keeps the framework-side service out of this crate's
    /// graph, so the wire contract is exercised against a canned peer.
    fn spawn_canned_server(frame_count: u64) -> (SocketAddr, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind canned devtools server");
        let addr = listener.local_addr().expect("canned server addr");
        let handle = thread::spawn(move || {
            // The bridge opens a short-lived reachability probe before its
            // real connection (see `connect_and_pump`), so accept in a loop
            // and keep going until a connection actually speaks the protocol;
            // the bound keeps a wedged test from hanging the suite.
            for stream in listener.incoming().take(MAX_CANNED_CONNECTIONS) {
                let Ok(stream) = stream else { return };
                if serve_canned(stream, frame_count) {
                    return;
                }
            }
        });
        (addr, handle)
    }

    /// How many connections [`spawn_canned_server`] will accept before giving
    /// up — the reachability probe plus the real connection, with slack.
    const MAX_CANNED_CONNECTIONS: usize = 4;

    /// Serve one connection; returns whether it spoke the protocol at all (a
    /// bare reachability probe, which opens and closes without writing, does
    /// not).
    fn serve_canned(stream: TcpStream, frame_count: u64) -> bool {
        let mut served = false;
        let mut write = stream.try_clone().expect("clone canned server stream");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            served = true;
            let request = match decode_line(line.trim_end()) {
                Ok(Incoming::Request(request)) => request,
                _ => {
                    line.clear();
                    continue;
                }
            };
            line.clear();
            let method = request.method.as_str();
            if method == Method::Handshake.as_str() {
                let token = request
                    .params
                    .get("token")
                    .and_then(serde_json::Value::as_str);
                let response = if token == Some(TOKEN) {
                    Response::success(
                        request.id,
                        serde_json::to_value(HandshakeInfo {
                            app_name: "huddle".to_string(),
                            frust_version: "0.1.0".to_string(),
                            protocol_version: PROTOCOL_VERSION,
                            capabilities: vec![Capability::FrameStats],
                        })
                        .expect("encode handshake result"),
                    )
                } else {
                    Response::error(
                        request.id,
                        RpcError::unauthorized("a valid devtools token is required"),
                    )
                };
                if write_line(&mut write, &encode_line(&response)).is_err() {
                    return served;
                }
                if token != Some(TOKEN) {
                    return served;
                }
            } else if method == Method::FrameStatsSubscribe.as_str() {
                let response = Response::success(request.id, serde_json::json!({ "ok": true }));
                if write_line(&mut write, &encode_line(&response)).is_err() {
                    return served;
                }
                for n in 0..frame_count {
                    let note = Notification::new(
                        Method::FrameStats.as_str(),
                        serde_json::to_value(frame(n)).expect("encode frame stats"),
                    );
                    if write_line(&mut write, &encode_line(&note)).is_err() {
                        return served;
                    }
                    // Spread the pushes across more than one coalescing
                    // window so the batching path is genuinely exercised.
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }
        served
    }

    fn write_line(stream: &mut TcpStream, line: &str) -> std::io::Result<()> {
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()
    }

    /// Drain messages until one satisfies `want`, or [`RECV_TIMEOUT`] passes.
    fn wait_for(
        rx: &mut UnboundedReceiver<Message>,
        want: impl Fn(&Message) -> bool,
    ) -> Option<Message> {
        let deadline = Instant::now() + RECV_TIMEOUT;
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(msg) if want(&msg) => return Some(msg),
                Ok(_) => {}
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
        None
    }

    fn target(session: SessionId, addr: SocketAddr, token: Option<&str>) -> DevtoolsTarget {
        DevtoolsTarget {
            session,
            port: addr.port(),
            token: token.map(str::to_string),
            android_serial: None,
        }
    }

    #[test]
    fn bridge_handshakes_forwards_frame_stats_and_disconnects_cleanly() {
        let (addr, server) = spawn_canned_server(6);
        let (tx, mut rx) = unbounded_channel();
        let mut bridge = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let session = SessionId(7);
        bridge.connect(target(session, addr, Some(TOKEN)), tx);

        let connected = wait_for(&mut rx, |msg| {
            matches!(msg, Message::DevtoolsConn(_, ConnEvent::Connected { .. }))
        })
        .expect("a Connected report");
        let Message::DevtoolsConn(got_session, ConnEvent::Connected { app_name, caps }) = connected
        else {
            unreachable!("filtered above")
        };
        assert_eq!(got_session, session);
        assert_eq!(app_name, "huddle");
        assert_eq!(caps, vec![Capability::FrameStats]);

        let frames = wait_for(&mut rx, |msg| {
            matches!(msg, Message::DevtoolsConn(_, ConnEvent::Frames(_)))
        })
        .expect("a frame-stats batch");
        let Message::DevtoolsConn(_, ConnEvent::Frames(batch)) = frames else {
            unreachable!("filtered above")
        };
        assert!(!batch.is_empty(), "a forwarded batch is never empty");
        assert!(
            batch.len() <= MAX_BATCH_FRAMES,
            "a batch never exceeds its cap"
        );

        // A clean disconnect stops and joins the thread — no report is
        // required afterwards, and nothing leaks.
        bridge.disconnect(session);
        drop(bridge);
        let _ = server.join();
    }

    #[test]
    fn a_rejected_token_surfaces_as_a_failure() {
        let (addr, server) = spawn_canned_server(0);
        let (tx, mut rx) = unbounded_channel();
        let mut bridge = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let session = SessionId(1);
        bridge.connect(target(session, addr, Some("wrong-token")), tx);

        let failed = wait_for(&mut rx, |msg| {
            matches!(msg, Message::DevtoolsConn(_, ConnEvent::Failed(_)))
        })
        .expect("a Failed report");
        let Message::DevtoolsConn(_, ConnEvent::Failed(error)) = failed else {
            unreachable!("filtered above")
        };
        assert!(
            error.contains("handshake"),
            "the failure names what was rejected: {error}"
        );
        drop(bridge);
        let _ = server.join();
    }

    #[test]
    fn an_unreachable_service_surfaces_as_a_failure_rather_than_hanging() {
        // Bind, note the port, then drop the listener: the port is (almost
        // certainly) refused, which is exactly the "service already exited"
        // case §B12's failed screen exists for.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);

        let (tx, mut rx) = unbounded_channel();
        let mut bridge = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        bridge.connect(target(SessionId(2), addr, Some(TOKEN)), tx);

        let failed = wait_for(&mut rx, |msg| {
            matches!(msg, Message::DevtoolsConn(_, ConnEvent::Failed(_)))
        });
        assert!(failed.is_some(), "an unreachable service reports Failed");
    }
}
