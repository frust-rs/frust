//! One background sampling thread per session with a resolved Android
//! identity (workbook §B12's System/Network tabs) — [`MetricsBridge`] owns
//! each session's `frust_drive::metrics::SamplerHandle` and drains it into
//! coalesced [`Message::DevtoolsMetrics`] batches.
//!
//! # Why a sibling module, not [`super::DevtoolsBridge`]
//!
//! [`super::DevtoolsBridge`] owns a *devtools-protocol* connection: a
//! blocking `TcpStream` handshake, an `adb forward`, and a frame-stats
//! subscription against the app's in-process devtools listener — reachable
//! only once a discovery line has been parsed off the session's log, and
//! gone entirely once the app's build compiles the listener out
//! (`DevtoolsLaunch::capable`).
//!
//! Metrics sampling is a different kind of thing: local reads of
//! `/proc`/`/sys` (desktop) or `adb shell cat`/`dumpsys` (Android) driven by
//! `frust_drive::metrics::MetricsSampler` — no socket, no handshake, no
//! build-feature gate (it works against a release build too), started once
//! a session's *pid* is known rather than once a discovery line lands.
//! Folding it into `DevtoolsBridge` would tangle two independently-lived
//! background threads with different start conditions and different
//! dependencies (`frust_drive::devtools_client` vs. `frust_drive::metrics`)
//! behind one struct; a sibling module with the same one-thread-per-session
//! shape keeps each bridge's own lifecycle legible and leaves
//! `devtools_bridge.rs` — already substantial — unchanged.
//!
//! # Retention
//!
//! Mirrors `DevtoolsBridge`'s connection-retention policy
//! (`crate::engine::update`'s module doc on [`super::session`]): once
//! started, a session's sampler runs for the rest of the session's life
//! regardless of whether DevTools is open or which tab is showing — cheap (a
//! periodic `/proc`/`adb` read, not a held socket), and it means reopening
//! DevTools (or switching back to System/Network) never re-pays a cold
//! start. It stops in exactly one place: the session ending
//! ([`MetricsBridge::stop`], driven by `crate::engine::Effect::MetricsStop`).
//!
//! # Coalescing
//!
//! Mirrors [`super::devtools_bridge`]'s frame-stats pump in shape (batch for
//! a window, cap the batch, drop the oldest beyond the cap) but not in
//! mechanism: [`frust_drive::metrics::MetricsReceiver`] exposes only
//! `recv`/`try_recv` (no `recv_timeout`, unlike the frame-stats
//! `mpsc::Receiver`), so the drain loop polls in short slices — the same
//! shape `MetricsSampler`'s own internal stop-wait uses — rather than
//! blocking on a timed receive.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use frust_drive::metrics::{MetricsSample, MetricsSampler, MetricsSource, MetricsTryRecvError};
use frust_drive::process::ProcessRunner;
use tokio::sync::mpsc::UnboundedSender;

use crate::engine::{Message, MetricsTarget};

use super::session::SessionId;

/// How long the drain loop gathers samples before forwarding a batch — the
/// same order of magnitude as `devtools_bridge::COALESCE_WINDOW`, generous
/// against [`MetricsSampler::MIN_INTERVAL`]'s tick floor.
const COALESCE_WINDOW: Duration = Duration::from_millis(200);

/// Cap on one forwarded batch (mirrors `devtools_bridge::MAX_BATCH_FRAMES`'s
/// drop-oldest shape) — [`crate::engine::METRICS_RING_CAP`] bounds what is
/// retained downstream regardless.
const MAX_BATCH_SAMPLES: usize = 240;

/// The sampler's own tick interval — once a second is plenty for a
/// sparkline a human is watching.
const SAMPLE_INTERVAL: Duration = MetricsSampler::DEFAULT_INTERVAL;

/// The ring capacity handed to [`MetricsSampler::spawn`] — generous against
/// [`COALESCE_WINDOW`] so a slow drain tick never loses samples before this
/// thread gets to them.
const SAMPLER_CAPACITY: usize = 64;

/// How finely the drain loop polls for a new sample / the stop flag, absent
/// a `recv_timeout` on [`frust_drive::metrics::MetricsReceiver`] — see the
/// module doc's Coalescing section.
const POLL_SLICE: Duration = Duration::from_millis(20);

/// Owns one metrics-sampling thread per session, keyed by [`SessionId`].
pub struct MetricsBridge {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    conns: HashMap<SessionId, Conn>,
}

/// One live (or finishing) sampler thread.
struct Conn {
    /// Set to ask the thread to wind down at its next poll slice.
    stop: Arc<AtomicBool>,
    /// Joined on stop/drop so no thread — and no background `adb`/`/proc`
    /// polling — outlives the bridge.
    worker: Option<JoinHandle<()>>,
}

impl MetricsBridge {
    /// Build a bridge over `runner` (the same [`ProcessRunner`] seam
    /// `DevtoolsBridge`/`Supervisor` use — every `adb` invocation goes
    /// through it, so a test can script it).
    pub fn new(runner: Arc<dyn ProcessRunner + Send + Sync>) -> Self {
        Self {
            runner,
            conns: HashMap::new(),
        }
    }

    /// Start sampling `target.session` against its resolved Android
    /// identity, replacing any sampler this bridge already holds for it (a
    /// session's identity, once resolved, never changes — this only
    /// protects against a caller that starts a session twice). Returns
    /// immediately; the sampling and draining happen on the spawned thread.
    pub fn start(&mut self, target: MetricsTarget, tx: UnboundedSender<Message>) {
        let session = target.session;
        self.stop(session);

        let stop = Arc::new(AtomicBool::new(false));
        let runner = Arc::clone(&self.runner);
        let worker = {
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name(format!("frust-tui-metrics-{}", session.0))
                .spawn(move || run_sampler(session, target, runner, stop, tx))
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
                // Spawning failed (a resource limit) — logged rather than
                // reported into the engine: unlike a devtools connection
                // failure there is no §B12 "failed" screen for metrics
                // sampling to show, only the System/Network tabs' existing
                // `SamplingState::Off` (which never flips to `On`, since no
                // `DevtoolsMetrics` batch will ever arrive).
                eprintln!(
                    "frust-tui: failed to start the metrics sampler thread for session {}: {err}",
                    session.0
                );
            }
        }
    }

    /// Tear down `session`'s sampler, if any: flag the thread, join it, and
    /// forget it. Idempotent; an unknown session is ignored.
    pub fn stop(&mut self, session: SessionId) {
        if let Some(mut conn) = self.conns.remove(&session) {
            conn.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = conn.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// Tear down every sampler (the bridge's job on quit).
    pub fn stop_all(&mut self) {
        let sessions: Vec<SessionId> = self.conns.keys().copied().collect();
        for session in sessions {
            self.stop(session);
        }
    }
}

impl Drop for MetricsBridge {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// One sampler thread: spawn `frust_drive::metrics::MetricsSampler` against
/// `target`'s Android identity, drain it into coalesced batches until asked
/// to stop, then stop the sampler cleanly on the way out.
fn run_sampler(
    session: SessionId,
    target: MetricsTarget,
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    stop: Arc<AtomicBool>,
    tx: UnboundedSender<Message>,
) {
    let source = MetricsSource::AndroidPkg {
        runner,
        serial: target.serial,
        pid: target.pid,
        pkg: target.pkg,
    };
    let mut handle = MetricsSampler::spawn(source, SAMPLE_INTERVAL, SAMPLER_CAPACITY);

    let mut batch: Vec<MetricsSample> = Vec::new();
    let mut window_start = Instant::now();
    'drain: loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        loop {
            match handle.samples.try_recv() {
                Ok(sample) => {
                    batch.push(sample);
                    if batch.len() >= MAX_BATCH_SAMPLES {
                        break;
                    }
                }
                Err(MetricsTryRecvError::Empty) => break,
                // The sampler thread wound down on its own (only reachable
                // if the source is `Unsupported` and this thread's own
                // `stop()` below hasn't run yet — in practice never, since
                // this thread owns the only handle) — flush and exit.
                Err(MetricsTryRecvError::Disconnected) => {
                    flush(&mut batch, session, &tx);
                    break 'drain;
                }
            }
        }
        if batch.len() >= MAX_BATCH_SAMPLES || window_start.elapsed() >= COALESCE_WINDOW {
            if !flush(&mut batch, session, &tx) {
                // The engine dropped its receiver (shutdown) — nothing left
                // to report to.
                break;
            }
            window_start = Instant::now();
        }
        thread::sleep(POLL_SLICE);
    }
    flush(&mut batch, session, &tx);
    handle.stop();
}

/// Forward `batch` (if non-empty) and clear it. Returns whether the engine is
/// still listening.
fn flush(
    batch: &mut Vec<MetricsSample>,
    session: SessionId,
    tx: &UnboundedSender<Message>,
) -> bool {
    if batch.is_empty() {
        return !tx.is_closed();
    }
    tx.send(Message::DevtoolsMetrics(session, std::mem::take(batch)))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::process::{FakeProcessRunner, Output};
    use std::time::Instant as StdInstant;
    use tokio::sync::mpsc::unbounded_channel;

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    const STAT_LINE: &str = "4021 (frustbench) S 1 4021 4021 0 -1 4194560 227 0 0 0 42 17 0 0 20 0 4 0 12345678 123456789 1234 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0 0 0 0 0 0 0 0 0";
    const MEMINFO_EXCERPT: &str = "\n           TOTAL PSS:    12345            TOTAL RSS:   54321       TOTAL SWAP PSS:      0\n";
    const NET_DEV: &str = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo: 100 0 0 0 0 0 0 0 100 0 0 0 0 0 0 0\n  eth0: 5000 0 0 0 0 0 0 0 3000 0 0 0 0 0 0 0\n";

    /// A fully-scripted fake `adb` source — mirrors
    /// `frust_drive::metrics::sampler`'s own test fixture, since this bridge
    /// exercises the identical `MetricsSource::AndroidPkg` probe set.
    fn fake_runner() -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with("adb -s emulator-5554 shell cat /proc/4021/stat", ok(STAT_LINE))
            .with(
                "adb -s emulator-5554 shell dumpsys meminfo it.f0x.frustbench",
                ok(MEMINFO_EXCERPT),
            )
            .with(
                "adb -s emulator-5554 shell for z in /sys/class/thermal/thermal_zone*; do t=$(cat \"$z/type\" 2>/dev/null); v=$(cat \"$z/temp\" 2>/dev/null); echo \"$z|$t|$v\"; done",
                ok("/sys/class/thermal/thermal_zone0|cpu-0|41000\n"),
            )
            .with("adb -s emulator-5554 shell cat /proc/net/dev", ok(NET_DEV))
    }

    fn target(session: SessionId) -> MetricsTarget {
        MetricsTarget {
            session,
            serial: "emulator-5554".to_string(),
            pid: "4021".to_string(),
            pkg: "it.f0x.frustbench".to_string(),
        }
    }

    /// A generous ceiling so a genuinely wedged test fails instead of
    /// hanging the suite.
    const RECV_TIMEOUT: Duration = Duration::from_secs(30);

    fn wait_for_a_batch(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<Message>,
        want: SessionId,
    ) -> Vec<MetricsSample> {
        let deadline = StdInstant::now() + RECV_TIMEOUT;
        while StdInstant::now() < deadline {
            match rx.try_recv() {
                Ok(Message::DevtoolsMetrics(session, batch)) if session == want => return batch,
                Ok(_) => {}
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
        panic!("no metrics batch arrived within the test timeout");
    }

    #[test]
    fn a_started_sampler_delivers_coalesced_batches_then_stops_cleanly() {
        let (tx, mut rx) = unbounded_channel();
        let mut bridge = MetricsBridge::new(Arc::new(fake_runner()));
        let session = SessionId(3);
        bridge.start(target(session), tx);

        let batch = wait_for_a_batch(&mut rx, session);
        assert!(!batch.is_empty(), "a forwarded batch is never empty");
        assert!(
            batch.iter().any(|s| matches!(s, MetricsSample::Mem(_))),
            "the first tick's mem/thermal/net samples land in an early batch: {batch:?}"
        );

        bridge.stop(session);
        drop(bridge);
    }

    #[test]
    fn stopping_an_unknown_session_is_a_harmless_no_op() {
        let mut bridge = MetricsBridge::new(Arc::new(FakeProcessRunner::new()));
        bridge.stop(SessionId(99));
    }

    #[test]
    fn dropping_the_bridge_stops_every_running_sampler() {
        let (tx, mut rx) = unbounded_channel();
        let mut bridge = MetricsBridge::new(Arc::new(fake_runner()));
        let session = SessionId(5);
        bridge.start(target(session), tx);
        let _ = wait_for_a_batch(&mut rx, session);
        drop(bridge);
        // No panic/hang on drop is the assertion; a stray late batch (the
        // thread was already mid-flush) is harmless and ignored here.
    }
}
