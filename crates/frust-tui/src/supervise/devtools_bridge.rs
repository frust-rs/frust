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
//! # On-demand requests (the Inspector tab)
//!
//! Beside the frame-stats subscription, the thread serves *request/response*
//! pulls the Inspector tab asks for: [`DevtoolsBridge::fetch_tree`] and
//! [`DevtoolsBridge::fetch_props`] post a command down a per-connection
//! channel, and the pump serves whatever is queued at the top of every window
//! ([`serve_commands`]) using the same blocking [`DevtoolsClient`], reporting
//! results as [`InspectorEvent`]s. No extra thread is spawned per request:
//! that would be an unbounded fan-out driven by keystrokes, and the client's
//! reader thread keeps buffering frame stats meanwhile. The cost is that a
//! slow `widget_tree` round trip pauses frame *forwarding* for its duration
//! (bounded by [`REQUEST_TIMEOUT`]) — the samples themselves are not lost,
//! they land in the client's mailbox and forward on the next window. A pull
//! that fails is reported and the connection keeps running: an error for one
//! node id is not a dead connection.
//!
//! # Lifetime
//!
//! A connection lives until it is explicitly disconnected (session end, or a
//! reconnect replacing it — see `crate::engine::update`'s retention policy),
//! the peer goes away (the client's frame-stats relay closes, surfaced as
//! [`ConnEvent::Failed`]), or the engine drops its receiver. [`Drop`] stops
//! and waits out every thread, so a torn-down bridge leaks neither a thread
//! nor an `adb` forward.
//!
//! # Teardown: mute, then wait elsewhere
//!
//! Disconnecting **mutes** the thread's report path ([`Sink`]) before handing
//! the thread back as a [`super::Teardown`], rather than joining it here.
//! Muting is what a join used to buy: a replaced connection can never race a
//! stale thread's late `Closed`/`Failed` into the engine for the same
//! session, because a muted thread's reports go nowhere at all (and it
//! notices the closed sink and winds itself down).
//!
//! Not joining is what keeps the workbench responsive: a disconnect otherwise
//! waits out the thread's current step on `crate::runner`'s event loop — one
//! [`COALESCE_WINDOW`] while pumping, but [`CONNECT_TIMEOUT`] or
//! [`REQUEST_TIMEOUT`] (seconds) while talking to a device that stopped
//! answering, with no repaint, input, or quit meanwhile. The caller places
//! that bounded wait instead (`crate::runner`: `spawn_blocking`), and past
//! [`super::TEARDOWN_DEADLINE`] the thread is detached — harmless now that it
//! is muted, and it still removes its own `adb` forward whenever it does
//! finish. The reachability probe stays explicitly bounded regardless: an
//! unbounded connect would leave a detached thread parked indefinitely.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use frust_devtools_protocol::FrameStats;
use frust_drive::devtools_client::{DevtoolsClient, adb_forward_ephemeral, adb_forward_remove};
use frust_drive::process::ProcessRunner;
use tokio::sync::mpsc::UnboundedSender;

use crate::engine::{ConnEvent, DevtoolsTarget, InspectorEvent, Message};

use super::session::SessionId;
use super::{TEARDOWN_DEADLINE, Teardown, spawn_tracked};

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

/// The engine channel one connection thread reports through, closable from
/// this side.
///
/// Setting it to `None` (the mute in the module doc's Teardown section) both
/// silences a thread that may outlive its disconnect and tells that thread to
/// wind down — every report path already treats "the engine stopped
/// listening" as a reason to return.
type Sink = Mutex<Option<UnboundedSender<Message>>>;

/// One live (or finishing) bridge thread.
struct Conn {
    /// Set to ask the thread to wind down at its next window tick.
    stop: Arc<AtomicBool>,
    /// On-demand Inspector pulls, served between frame windows. Unbounded,
    /// but the engine only ever has one tree pull and one props pull in
    /// flight per session ([`crate::engine::InspectorTab`]'s gating), so the
    /// queue is bounded by that in practice.
    commands: mpsc::Sender<BridgeCommand>,
    /// Muted on disconnect/drop, before the thread is waited out.
    sink: Arc<Sink>,
    /// Waited out (with a deadline) on disconnect/drop so no thread — and no
    /// `adb` forward — outlives the bridge for longer than its own wedged
    /// call.
    worker: JoinHandle<()>,
    /// Disconnects when `worker` returns — see [`spawn_tracked`].
    done: mpsc::Receiver<()>,
}

impl Conn {
    /// Signal the thread, mute it, and package it for a bounded wait
    /// elsewhere.
    fn into_teardown(self, session: SessionId) -> Teardown {
        self.stop.store(true, Ordering::SeqCst);
        *self.sink.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Teardown::new(
            format!("the devtools bridge for session {}", session.0),
            self.done,
            self.worker,
        )
    }
}

/// One on-demand request for a bridge thread to serve.
///
/// There is no `Shutdown` variant: teardown is [`Conn::stop`]'s job (checked
/// every window and joined by [`DevtoolsBridge::disconnect`]), and a second
/// teardown path with different timing would be a way for the two to
/// disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BridgeCommand {
    /// Pull the whole widget tree (`widget_tree`).
    FetchTree,
    /// Pull one node's props (`widget_props`).
    FetchProps(u64),
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
    ///
    /// The replaced connection (if any) comes back as a [`Teardown`] for the
    /// caller to wait out off the event loop — see [`Self::disconnect`].
    pub fn connect(
        &mut self,
        target: DevtoolsTarget,
        tx: UnboundedSender<Message>,
    ) -> Option<Teardown> {
        let session = target.session;
        let replaced = self.disconnect(session);

        let stop = Arc::new(AtomicBool::new(false));
        let runner = Arc::clone(&self.runner);
        let (commands, command_rx) = mpsc::channel::<BridgeCommand>();
        let sink: Arc<Sink> = Arc::new(Mutex::new(Some(tx.clone())));
        let spawned = {
            let stop = Arc::clone(&stop);
            let sink = Arc::clone(&sink);
            spawn_tracked(format!("frust-tui-devtools-{}", session.0), move || {
                run_bridge(target, runner, stop, command_rx, &sink)
            })
        };
        match spawned {
            Ok((worker, done)) => {
                self.conns.insert(
                    session,
                    Conn {
                        stop,
                        commands,
                        sink,
                        worker,
                        done,
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
        replaced
    }

    /// Ask `session`'s bridge for a fresh `widget_tree` snapshot; the result
    /// comes back as [`Message::DevtoolsInspector`].
    pub fn fetch_tree(&self, session: SessionId, tx: &UnboundedSender<Message>) {
        self.request(session, BridgeCommand::FetchTree, tx);
    }

    /// Ask `session`'s bridge for one node's `widget_props`, reported the same
    /// way as [`Self::fetch_tree`].
    pub fn fetch_props(&self, session: SessionId, id: u64, tx: &UnboundedSender<Message>) {
        self.request(session, BridgeCommand::FetchProps(id), tx);
    }

    /// Queue one command for `session`'s thread. A session with no live
    /// bridge (never connected, or its thread already wound down) reports an
    /// [`InspectorEvent::Failed`] instead of dropping the request silently —
    /// the engine is holding an in-flight flag for it, and a request that
    /// never answers would wedge that tab.
    fn request(&self, session: SessionId, command: BridgeCommand, tx: &UnboundedSender<Message>) {
        let delivered = self
            .conns
            .get(&session)
            .is_some_and(|conn| conn.commands.send(command).is_ok());
        if !delivered {
            let _ = tx.send(Message::DevtoolsInspector(
                session,
                InspectorEvent::Failed(
                    "the devtools connection for this session is no longer open".to_string(),
                ),
            ));
        }
    }

    /// Tear down `session`'s connection, if any: flag the thread, mute it,
    /// forget it, and hand it back as a [`Teardown`] the caller waits out
    /// *off* the event loop (`crate::runner`: `spawn_blocking`). The thread
    /// removes its own `adb forward` on the way out. Idempotent; an unknown
    /// session yields `None`.
    ///
    /// Signalling and muting are what happen here — never joining; see the
    /// module doc's Teardown section for why a join on this side is a
    /// seconds-long workbench freeze on an unresponsive device.
    pub fn disconnect(&mut self, session: SessionId) -> Option<Teardown> {
        Some(self.conns.remove(&session)?.into_teardown(session))
    }

    /// Tear down every connection (the bridge's job on quit).
    ///
    /// This is the one place the wait is taken inline — there is no loop left
    /// to protect by then — and every thread is signalled before any of them
    /// is waited on, so the whole set shares a single
    /// [`TEARDOWN_DEADLINE`] rather than paying one per session.
    pub fn disconnect_all(&mut self) {
        let sessions: Vec<SessionId> = self.conns.keys().copied().collect();
        let mut teardowns: Vec<Teardown> = sessions
            .into_iter()
            .filter_map(|session| self.disconnect(session))
            .collect();
        let deadline = Instant::now() + TEARDOWN_DEADLINE;
        for teardown in &mut teardowns {
            teardown.wait_until(deadline);
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
    commands: mpsc::Receiver<BridgeCommand>,
    tx: &Sink,
) {
    let session = target.session;
    let forward = match target.android_serial.as_deref() {
        Some(serial) => match adb_forward_ephemeral(runner.as_ref(), serial, target.port) {
            Ok(local_port) => Some(Forward {
                serial: serial.to_string(),
                local_port,
            }),
            Err(err) => {
                report(tx, session, ConnEvent::Failed(format!("{err:#}")));
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
    let result = connect_and_pump(addr, target.token.as_deref(), session, &stop, &commands, tx);

    if let Some(forward) = forward {
        // Best-effort: the connection is over either way, and a failure to
        // remove the forward must not mask why it ended.
        let _ = adb_forward_remove(runner.as_ref(), &forward.serial, forward.local_port);
    }

    let event = match result {
        Ok(()) => ConnEvent::Closed,
        Err(err) => ConnEvent::Failed(format!("{err:#}")),
    };
    report(tx, session, event);
}

/// Connect, handshake, arm the frame-stats subscription, and pump batches
/// until asked to stop (`Ok`) or the connection/engine goes away (`Err` /
/// `Ok` respectively — see [`pump_frames`]).
fn connect_and_pump(
    addr: SocketAddr,
    token: Option<&str>,
    session: SessionId,
    stop: &AtomicBool,
    commands: &mpsc::Receiver<BridgeCommand>,
    tx: &Sink,
) -> Result<()> {
    // A bounded reachability probe first. `DevtoolsClient::connect` uses the
    // platform's own TCP connect behavior, which has no bound this side can
    // set, so an unbounded connect here would be a thread that never notices
    // its stop flag — detached at teardown and parked for as long as the
    // platform's own connect takes. Probing with an explicit timeout puts the
    // only wait that can actually hang under [`CONNECT_TIMEOUT`]; the
    // client's own connect that follows is then to an already-proven-
    // reachable loopback port.
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
    pump_frames(&client, &frames, commands, session, stop, tx)
}

/// Forward coalesced frame-stats batches — serving any queued on-demand
/// request first — until `stop` is set (`Ok`), the engine drops its receiver
/// (`Ok` — nothing left to report to), or the connection closes (`Err`, which
/// surfaces §B12's failed screen).
///
/// The window is measured, not inferred from an idle receive: each batch is
/// forwarded once [`COALESCE_WINDOW`] has *elapsed* since the last one,
/// whatever the receive did meanwhile. Flushing only when a receive times out
/// would starve exactly the case DevTools exists for — an app publishing
/// faster than the window (>30fps) never leaves the receiver idle, so its
/// batch would grow to [`MAX_BATCH_FRAMES`], silently shed its oldest samples,
/// and reach the Performance tab only once the app went idle.
fn pump_frames(
    client: &DevtoolsClient,
    frames: &mpsc::Receiver<FrameStats>,
    commands: &mpsc::Receiver<BridgeCommand>,
    session: SessionId,
    stop: &AtomicBool,
    tx: &Sink,
) -> Result<()> {
    let mut batch: Vec<FrameStats> = Vec::new();
    let mut window_start = Instant::now();
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Served at the top of every iteration rather than only on the
        // window's timeout branch: an app publishing faster than
        // `COALESCE_WINDOW` iterates per frame, and a request must not wait on
        // a busy app to go idle.
        if !serve_commands(client, commands, session, tx) {
            return Ok(());
        }
        // Only ever wait out the *rest* of the current window, so a steady
        // stream of frames can't push the flush below past it.
        let remaining = COALESCE_WINDOW.saturating_sub(window_start.elapsed());
        match frames.recv_timeout(remaining) {
            Ok(stats) => {
                batch.push(stats);
                if batch.len() > MAX_BATCH_FRAMES {
                    batch.remove(0);
                }
            }
            // Nothing arrived in the rest of the window — an idle app simply
            // forwards nothing below.
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // The client's reader thread closed the frame-stats mailbox: the
            // peer is gone.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                flush(&mut batch, session, tx);
                anyhow::bail!("the devtools connection closed");
            }
        }
        if window_start.elapsed() >= COALESCE_WINDOW {
            if !flush(&mut batch, session, tx) {
                return Ok(());
            }
            window_start = Instant::now();
        }
    }
}

/// Forward `batch` (if non-empty) and clear it. Returns whether the engine is
/// still listening.
fn flush(batch: &mut Vec<FrameStats>, session: SessionId, tx: &Sink) -> bool {
    if batch.is_empty() {
        return listening(tx);
    }
    report(tx, session, ConnEvent::Frames(std::mem::take(batch)))
}

/// Post one report into the engine channel. Returns whether it was delivered
/// — a muted sink (this connection was disconnected) or a closed channel (the
/// engine is gone) both mean the caller should wind down.
fn report(tx: &Sink, session: SessionId, event: ConnEvent) -> bool {
    send(tx, Message::DevtoolsConn(session, event))
}

/// [`report`] for the Inspector's own request/response channel.
fn report_inspector(tx: &Sink, session: SessionId, event: InspectorEvent) -> bool {
    send(tx, Message::DevtoolsInspector(session, event))
}

/// The one place a bridge thread touches the engine channel, so muting a
/// connection ([`Conn::into_teardown`]) is enough to silence it everywhere.
fn send(sink: &Sink, message: Message) -> bool {
    let sink = sink.lock().unwrap_or_else(|p| p.into_inner());
    sink.as_ref().is_some_and(|tx| tx.send(message).is_ok())
}

/// Whether reports would still reach the engine, without posting one.
fn listening(sink: &Sink) -> bool {
    let sink = sink.lock().unwrap_or_else(|p| p.into_inner());
    sink.as_ref().is_some_and(|tx| !tx.is_closed())
}

/// Serve every request queued right now (never blocking on an empty queue).
/// Returns whether the engine is still listening.
fn serve_commands(
    client: &DevtoolsClient,
    commands: &mpsc::Receiver<BridgeCommand>,
    session: SessionId,
    tx: &Sink,
) -> bool {
    loop {
        match commands.try_recv() {
            Ok(command) => {
                if !serve_command(client, command, session, tx) {
                    return false;
                }
            }
            // Empty: nothing to do this window. Disconnected: the bridge
            // dropped this connection's sender, which only happens on
            // teardown — the `stop` flag the loop already checks is the
            // authority on winding down, so this just stops serving.
            Err(_) => return true,
        }
    }
}

/// Serve one request over the blocking client. A rejection (a vanished node
/// id, an unauthorized connection) is reported as
/// [`InspectorEvent::Failed`] and the pump carries on — it says nothing about
/// the connection's health, so it must not tear it down.
fn serve_command(
    client: &DevtoolsClient,
    command: BridgeCommand,
    session: SessionId,
    tx: &Sink,
) -> bool {
    let event = match command {
        BridgeCommand::FetchTree => match client.widget_tree() {
            Ok(dump) => InspectorEvent::TreeArrived(dump),
            Err(err) => InspectorEvent::Failed(format!("{err:#}")),
        },
        BridgeCommand::FetchProps(id) => match client.widget_props(id) {
            Ok(props) => InspectorEvent::PropsArrived(id, props),
            Err(err) => InspectorEvent::Failed(format!("{err:#}")),
        },
    };
    report_inspector(tx, session, event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_devtools_protocol::{
        Capability, HandshakeInfo, Incoming, Method, Notification, PROTOCOL_VERSION, Response,
        RpcError, WidgetNode, WidgetProps, WidgetTreeDump, decode_line, encode_line, serde_json,
    };
    use frust_drive::process::FakeProcessRunner;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicU64;
    use std::thread;
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

    /// How the canned server publishes `frame_stats` after a subscribe.
    #[derive(Clone)]
    struct PushPlan {
        /// Stop after this many notifications; `None` pushes until [`stop`]
        /// is set, which is what a *continuously rendering* app looks like.
        limit: Option<u64>,
        /// Gap between pushes.
        gap: Duration,
        /// How many have actually been written — the server-side witness a
        /// test counts batches against.
        pushed: Arc<AtomicU64>,
        /// Set by the test to end an unlimited push loop.
        stop: Arc<AtomicBool>,
    }

    impl PushPlan {
        /// Push `count` frames, spread across more than one coalescing window
        /// so the batching path is genuinely exercised.
        fn fixed(count: u64) -> Self {
            Self {
                limit: Some(count),
                gap: Duration::from_millis(20),
                pushed: Arc::new(AtomicU64::new(0)),
                stop: Arc::new(AtomicBool::new(false)),
            }
        }

        /// Push every `gap` until told to stop.
        fn continuous(gap: Duration) -> Self {
            Self {
                limit: None,
                gap,
                pushed: Arc::new(AtomicU64::new(0)),
                stop: Arc::new(AtomicBool::new(false)),
            }
        }

        fn pushed(&self) -> u64 {
            self.pushed.load(Ordering::SeqCst)
        }

        fn end(&self) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    /// A hand-rolled NDJSON devtools server: accepts one connection, answers
    /// `handshake` (rejecting a wrong/missing token exactly as the real
    /// service does), `frame_stats_subscribe` (after which it pushes
    /// `frame_stats` notifications per `plan` *from its own thread*, so
    /// requests keep being served while frames flow), `widget_tree` and
    /// `widget_props`. Deliberately *not* `frust-devtools` — the tooling
    /// charter keeps the framework-side service out of this crate's graph, so
    /// the wire contract is exercised against a canned peer.
    fn spawn_canned_server(plan: PushPlan) -> (SocketAddr, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind canned devtools server");
        let addr = listener.local_addr().expect("canned server addr");
        let handle = thread::spawn(move || {
            // The bridge opens a short-lived reachability probe before its
            // real connection (see `connect_and_pump`), so accept in a loop
            // and keep going until a connection actually speaks the protocol;
            // the bound keeps a wedged test from hanging the suite.
            for stream in listener.incoming().take(MAX_CANNED_CONNECTIONS) {
                let Ok(stream) = stream else { return };
                if serve_canned(stream, plan.clone()) {
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
    fn serve_canned(stream: TcpStream, plan: PushPlan) -> bool {
        let mut served = false;
        // Shared because the frame pusher below writes from its own thread:
        // two writers interleaving mid-line would corrupt the NDJSON stream.
        let write = Arc::new(Mutex::new(
            stream.try_clone().expect("clone canned server stream"),
        ));
        let mut pusher: Option<thread::JoinHandle<()>> = None;
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
                            capabilities: vec![Capability::FrameStats, Capability::WidgetTree],
                        })
                        .expect("encode handshake result"),
                    )
                } else {
                    Response::error(
                        request.id,
                        RpcError::unauthorized("a valid devtools token is required"),
                    )
                };
                if write_line(&write, &encode_line(&response)).is_err() {
                    break;
                }
                if token != Some(TOKEN) {
                    break;
                }
            } else if method == Method::FrameStatsSubscribe.as_str() {
                let response = Response::success(request.id, serde_json::json!({ "ok": true }));
                if write_line(&write, &encode_line(&response)).is_err() {
                    break;
                }
                let write = Arc::clone(&write);
                let plan = plan.clone();
                pusher = Some(thread::spawn(move || {
                    let mut n = 0;
                    while plan.limit.is_none_or(|limit| n < limit)
                        && !plan.stop.load(Ordering::SeqCst)
                    {
                        let note = Notification::new(
                            Method::FrameStats.as_str(),
                            serde_json::to_value(frame(n)).expect("encode frame stats"),
                        );
                        if write_line(&write, &encode_line(&note)).is_err() {
                            return;
                        }
                        plan.pushed.fetch_add(1, Ordering::SeqCst);
                        n += 1;
                        thread::sleep(plan.gap);
                    }
                }));
            } else if method == Method::WidgetTree.as_str() {
                let response = Response::success(
                    request.id,
                    serde_json::to_value(canned_tree()).expect("encode widget tree"),
                );
                if write_line(&write, &encode_line(&response)).is_err() {
                    break;
                }
            } else if method == Method::WidgetProps.as_str() {
                let id = request
                    .params
                    .get("id")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let response = Response::success(
                    request.id,
                    serde_json::to_value(WidgetProps {
                        id,
                        entries: vec![("axis".to_string(), "Vertical".to_string())],
                    })
                    .expect("encode widget props"),
                );
                if write_line(&write, &encode_line(&response)).is_err() {
                    break;
                }
            }
        }
        if let Some(pusher) = pusher {
            let _ = pusher.join();
        }
        served
    }

    /// The canned server's fixed two-node tree: `Column #1 > Text #2`.
    fn canned_tree() -> WidgetTreeDump {
        WidgetTreeDump {
            roots: vec![WidgetNode {
                id: 1,
                type_name: "frust_widgets::flex::FlexWidget".to_string(),
                debug_label: None,
                bounds: None,
                children: vec![WidgetNode {
                    id: 2,
                    type_name: "frust_widgets::text::TextWidget".to_string(),
                    debug_label: Some("greeting".to_string()),
                    bounds: None,
                    children: Vec::new(),
                }],
            }],
        }
    }

    fn write_line(stream: &Mutex<TcpStream>, line: &str) -> std::io::Result<()> {
        let mut stream = stream.lock().unwrap_or_else(|p| p.into_inner());
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()
    }

    /// Take the wait a caller would normally hand to `spawn_blocking`.
    fn settle(teardown: Option<Teardown>) {
        if let Some(teardown) = teardown {
            teardown.wait();
        }
    }

    /// Wait for the next forwarded frame-stats batch.
    fn wait_for_frames(rx: &mut UnboundedReceiver<Message>) -> Vec<FrameStats> {
        let msg = wait_for(rx, |msg| {
            matches!(msg, Message::DevtoolsConn(_, ConnEvent::Frames(_)))
        })
        .expect("a frame-stats batch");
        let Message::DevtoolsConn(_, ConnEvent::Frames(batch)) = msg else {
            unreachable!("filtered above")
        };
        batch
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
        let (addr, server) = spawn_canned_server(PushPlan::fixed(6));
        let (tx, mut rx) = unbounded_channel();
        let mut bridge = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let session = SessionId(7);
        settle(bridge.connect(target(session, addr, Some(TOKEN)), tx));

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
        assert_eq!(
            caps,
            vec![Capability::FrameStats, Capability::WidgetTree],
            "the declared capability set is forwarded verbatim"
        );

        let batch = wait_for_frames(&mut rx);
        assert!(!batch.is_empty(), "a forwarded batch is never empty");
        assert!(
            batch.len() <= MAX_BATCH_FRAMES,
            "a batch never exceeds its cap"
        );

        // A clean disconnect stops, mutes and winds down the thread — no
        // report is required afterwards, and nothing leaks.
        settle(bridge.disconnect(session));
        drop(bridge);
        let _ = server.join();
    }

    #[test]
    fn frames_are_forwarded_every_window_while_the_app_keeps_publishing() {
        // A frame every 2ms is ~500fps: far faster than COALESCE_WINDOW, i.e.
        // the profiling case DevTools exists for. The server pushes until
        // *this test* stops it, so every batch observed below necessarily
        // arrived while frames were still flowing — the shape that used to
        // starve, because a receive that never idles never timed out and the
        // pump only flushed on a timeout.
        let plan = PushPlan::continuous(Duration::from_millis(2));
        let (addr, server) = spawn_canned_server(plan.clone());
        let (tx, mut rx) = unbounded_channel();
        let mut bridge = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let session = SessionId(21);
        settle(bridge.connect(target(session, addr, Some(TOKEN)), tx));
        wait_for(&mut rx, |msg| {
            matches!(msg, Message::DevtoolsConn(_, ConnEvent::Connected { .. }))
        })
        .expect("a Connected report");

        let first = wait_for_frames(&mut rx);
        let pushed_after_first = plan.pushed();
        let second = wait_for_frames(&mut rx);
        let pushed_after_second = plan.pushed();

        assert!(
            !first.is_empty() && !second.is_empty(),
            "a forwarded batch is never empty"
        );
        assert!(
            first.len() <= MAX_BATCH_FRAMES && second.len() <= MAX_BATCH_FRAMES,
            "a batch never exceeds its cap"
        );
        assert!(
            second[0].n > first[first.len() - 1].n,
            "consecutive windows carry consecutive frames, oldest first: \
             {first:?} then {second:?}"
        );
        assert!(
            pushed_after_second > pushed_after_first,
            "the app kept publishing across both windows ({pushed_after_first} \
             then {pushed_after_second} frames pushed)"
        );

        plan.end();
        settle(bridge.disconnect(session));
        drop(bridge);
        let _ = server.join();
    }

    #[test]
    fn a_rejected_token_surfaces_as_a_failure() {
        let (addr, server) = spawn_canned_server(PushPlan::fixed(0));
        let (tx, mut rx) = unbounded_channel();
        let mut bridge = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let session = SessionId(1);
        settle(bridge.connect(target(session, addr, Some("wrong-token")), tx));

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
    fn on_demand_pulls_are_served_between_frame_windows() {
        // Enough frames to still be flowing after both pulls have been served
        // (~20ms apart), so the interleaving is real rather than incidental.
        let (addr, server) = spawn_canned_server(PushPlan::fixed(200));
        let (tx, mut rx) = unbounded_channel();
        let mut bridge = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let session = SessionId(11);
        settle(bridge.connect(target(session, addr, Some(TOKEN)), tx.clone()));
        wait_for(&mut rx, |msg| {
            matches!(msg, Message::DevtoolsConn(_, ConnEvent::Connected { .. }))
        })
        .expect("a Connected report");

        bridge.fetch_tree(session, &tx);
        let tree = wait_for(&mut rx, |msg| {
            matches!(
                msg,
                Message::DevtoolsInspector(_, InspectorEvent::TreeArrived(_))
            )
        })
        .expect("a TreeArrived report");
        let Message::DevtoolsInspector(got_session, InspectorEvent::TreeArrived(dump)) = tree
        else {
            unreachable!("filtered above")
        };
        assert_eq!(got_session, session);
        assert_eq!(dump.roots.len(), 1);
        assert_eq!(
            dump.roots[0].children[0].debug_label.as_deref(),
            Some("greeting")
        );

        bridge.fetch_props(session, 2, &tx);
        let props = wait_for(&mut rx, |msg| {
            matches!(
                msg,
                Message::DevtoolsInspector(_, InspectorEvent::PropsArrived(..))
            )
        })
        .expect("a PropsArrived report");
        let Message::DevtoolsInspector(_, InspectorEvent::PropsArrived(id, props)) = props else {
            unreachable!("filtered above")
        };
        assert_eq!(id, 2);
        assert_eq!(props.id, 2);
        assert!(!props.entries.is_empty());

        // The frame pump survived both round trips: a batch still lands after
        // them.
        assert!(
            wait_for(&mut rx, |msg| matches!(
                msg,
                Message::DevtoolsConn(_, ConnEvent::Frames(_))
            ))
            .is_some(),
            "frame stats keep flowing around an on-demand pull"
        );

        settle(bridge.disconnect(session));
        drop(bridge);
        let _ = server.join();
    }

    #[test]
    fn a_pull_for_a_session_with_no_bridge_reports_a_failure() {
        let (tx, mut rx) = unbounded_channel();
        let bridge = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        // Nothing was ever connected for this session: the request cannot be
        // served, and must say so rather than leave the tab's in-flight flag
        // set forever.
        bridge.fetch_tree(SessionId(42), &tx);
        let failed = wait_for(&mut rx, |msg| {
            matches!(
                msg,
                Message::DevtoolsInspector(_, InspectorEvent::Failed(_))
            )
        })
        .expect("a Failed report");
        assert!(matches!(failed, Message::DevtoolsInspector(s, _) if s == SessionId(42)));
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
        settle(bridge.connect(target(SessionId(2), addr, Some(TOKEN)), tx));

        let failed = wait_for(&mut rx, |msg| {
            matches!(msg, Message::DevtoolsConn(_, ConnEvent::Failed(_)))
        });
        assert!(failed.is_some(), "an unreachable service reports Failed");
    }
}
