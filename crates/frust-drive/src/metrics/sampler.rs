//! Background-thread metrics sampling — [`MetricsSampler::spawn`] and the
//! bounded, drop-oldest [`SamplerHandle::samples`] stream it hands back.
//!
//! Mirrors `process.rs`'s `spawn_streaming`/[`crate::process::StreamHandle`]
//! idioms deliberately: a `Mutex`+`Condvar`-backed ring buffer a producer
//! thread never blocks pushing into (a push at capacity drops the oldest
//! buffered sample rather than waiting for a consumer), and a receiver handle
//! shaped like `mpsc::Receiver` (`recv`/`try_recv`/`iter`). It departs from
//! that shape in one place: [`SamplerHandle::drop`] signals the background
//! thread to stop (not just detach) — a forgotten, undrained `StreamHandle`
//! is harmless (its child process runs to completion either way), but a
//! forgotten `SamplerHandle` would otherwise poll `/proc`/`adb` forever with
//! nobody reading the results.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::proc_stat::{self, CpuTicks};
use super::{MetricsSample, MetricsSource, fetch_raw, snapshot_into_samples};

/// Mirrors [`std::sync::mpsc::RecvError`] — [`MetricsReceiver::recv`]'s error
/// type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("metrics stream closed with no samples left buffered")]
pub struct MetricsRecvError;

/// Mirrors [`std::sync::mpsc::TryRecvError`] — [`MetricsReceiver::try_recv`]'s
/// error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MetricsTryRecvError {
    #[error("no sample buffered right now")]
    Empty,
    #[error("metrics stream closed with no samples left buffered")]
    Disconnected,
}

struct SampleBuffer {
    queue: VecDeque<MetricsSample>,
    closed: bool,
    dropped: u64,
    capacity: usize,
}

struct SampleBufferShared {
    state: Mutex<SampleBuffer>,
    ready: Condvar,
}

impl SampleBufferShared {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(SampleBuffer {
                queue: VecDeque::new(),
                closed: false,
                dropped: 0,
                capacity: capacity.max(1),
            }),
            ready: Condvar::new(),
        })
    }

    /// Producer side: pushes `sample`, dropping the oldest buffered sample
    /// (incrementing the dropped counter) if already at capacity. Never
    /// blocks — same no-deadlock contract as `process::LineBufferShared`.
    fn push(&self, sample: MetricsSample) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.queue.len() >= state.capacity {
            state.queue.pop_front();
            state.dropped += 1;
        }
        state.queue.push_back(sample);
        drop(state);
        self.ready.notify_one();
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        drop(state);
        self.ready.notify_all();
    }
}

/// A bounded, drop-oldest receive handle over a [`SamplerHandle`]'s buffered
/// samples — see the module doc.
#[derive(Clone)]
pub struct MetricsReceiver {
    shared: Arc<SampleBufferShared>,
}

impl MetricsReceiver {
    /// Blocks until a sample is available, or errors once the buffer is
    /// closed with nothing left queued.
    pub fn recv(&self) -> Result<MetricsSample, MetricsRecvError> {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(sample) = state.queue.pop_front() {
                return Ok(sample);
            }
            if state.closed {
                return Err(MetricsRecvError);
            }
            state = self
                .shared
                .ready
                .wait(state)
                .unwrap_or_else(|p| p.into_inner());
        }
    }

    /// Non-blocking: the next buffered sample, [`MetricsTryRecvError::Empty`]
    /// if none are buffered right now but the producer is still alive, or
    /// [`MetricsTryRecvError::Disconnected`] if closed with nothing left.
    pub fn try_recv(&self) -> Result<MetricsSample, MetricsTryRecvError> {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(sample) = state.queue.pop_front() {
            return Ok(sample);
        }
        if state.closed {
            Err(MetricsTryRecvError::Disconnected)
        } else {
            Err(MetricsTryRecvError::Empty)
        }
    }

    /// Blocking iterator over every remaining sample, ending once the buffer
    /// closes with nothing left queued.
    pub fn iter(&self) -> MetricsIter<'_> {
        MetricsIter { receiver: self }
    }

    /// The number of buffered samples dropped so far because the ring buffer
    /// was at capacity when a new sample arrived. Additive — never resets.
    pub fn dropped_samples(&self) -> u64 {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .dropped
    }
}

/// [`MetricsReceiver::iter`]'s iterator type.
pub struct MetricsIter<'a> {
    receiver: &'a MetricsReceiver,
}

impl Iterator for MetricsIter<'_> {
    type Item = MetricsSample;

    fn next(&mut self) -> Option<MetricsSample> {
        self.receiver.recv().ok()
    }
}

impl<'a> IntoIterator for &'a MetricsReceiver {
    type Item = MetricsSample;
    type IntoIter = MetricsIter<'a>;

    fn into_iter(self) -> MetricsIter<'a> {
        self.iter()
    }
}

/// A running background metrics-sampling thread, returned by
/// [`MetricsSampler::spawn`].
///
/// **Drop behavior: signal-and-detach, not join-on-drop.** Dropping a
/// `SamplerHandle` without calling [`stop`](Self::stop) never blocks — the
/// background thread is asked to stop (see the module doc) but the drop
/// itself doesn't wait for it to actually exit. A caller that needs to know
/// the thread has actually wound down (e.g. before reusing whatever `source`
/// referenced) must call [`stop`](Self::stop) explicitly.
pub struct SamplerHandle {
    /// The bounded, drop-oldest stream of samples this sampler produces.
    pub samples: MetricsReceiver,
    stop_flag: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl SamplerHandle {
    /// Signals the background thread to stop, then blocks until it has
    /// actually exited. **Idempotent** — a second call is a no-op.
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    /// The number of buffered samples dropped so far — see
    /// [`MetricsReceiver::dropped_samples`].
    pub fn dropped_samples(&self) -> u64 {
        self.samples.dropped_samples()
    }
}

impl Drop for SamplerHandle {
    fn drop(&mut self) {
        // Signal-and-detach — see the type's doc comment.
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

/// Slice a wait-for-the-next-tick sleep into, so [`SamplerHandle::stop`]
/// responds within one slice instead of waiting out a full tick interval.
const STOP_POLL_SLICE: Duration = Duration::from_millis(20);

/// Sleeps up to `interval` in [`STOP_POLL_SLICE`]-sized steps, returning
/// early (`true`) the moment `stop_flag` is observed set.
fn wait_or_stop(interval: Duration, stop_flag: &AtomicBool) -> bool {
    let mut waited = Duration::ZERO;
    while waited < interval {
        if stop_flag.load(Ordering::Relaxed) {
            return true;
        }
        let step = STOP_POLL_SLICE.min(interval - waited);
        thread::sleep(step);
        waited += step;
    }
    stop_flag.load(Ordering::Relaxed)
}

/// Spawns/configures background metrics sampling — see [`MetricsSampler::spawn`].
pub struct MetricsSampler;

impl MetricsSampler {
    /// The tick interval used when a caller has no stronger preference.
    pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);
    /// The floor every requested interval is clamped to. Sampling faster
    /// than this would spend more wall-clock time in `/proc`+`/sys` syscalls
    /// (or `adb` round-trips, which dominate) than actually elapses between
    /// ticks.
    pub const MIN_INTERVAL: Duration = Duration::from_millis(250);

    /// Starts a background thread sampling `source` every `interval`
    /// (clamped to at least [`MIN_INTERVAL`]), pushing each tick's samples
    /// into a bounded, drop-oldest ring buffer of `capacity` samples —
    /// [`SamplerHandle::samples`] is the draining side. The first tick fires
    /// immediately (no initial wait), so a caller sees its first sample
    /// promptly rather than after a whole interval.
    ///
    /// `capacity` is caller-chosen (unlike `process::LINE_BUFFER_CAP`'s
    /// single workspace-wide constant) since a metrics stream's useful
    /// history depends entirely on the consumer — a TUI panel graphing the
    /// last N points wants a small bound; a session recording a full run's
    /// history wants a much larger one.
    pub fn spawn(source: MetricsSource, interval: Duration, capacity: usize) -> SamplerHandle {
        let interval = interval.max(Self::MIN_INTERVAL);
        let shared = SampleBufferShared::new(capacity);
        let producer = Arc::clone(&shared);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_worker = Arc::clone(&stop_flag);

        let worker = thread::spawn(move || {
            let mut prev_cpu: Option<(CpuTicks, Instant)> = None;
            let started = Instant::now();

            loop {
                let now = Instant::now();
                if let Ok(snapshot) = fetch_raw(&source) {
                    let cpu_percent = match (&prev_cpu, snapshot.cpu_ticks) {
                        (Some((prev_ticks, prev_at)), Some(curr_ticks)) => {
                            Some(proc_stat::cpu_percent_from_ticks(
                                *prev_ticks,
                                curr_ticks,
                                now.duration_since(*prev_at),
                            ))
                        }
                        _ => None,
                    };
                    if let Some(curr_ticks) = snapshot.cpu_ticks {
                        prev_cpu = Some((curr_ticks, now));
                    }
                    // `at_ms` is relative to the sampler's own start
                    // (`Instant::now()` has no fixed epoch to key off
                    // otherwise) — see `TimestampMs`'s doc. Measured from
                    // `started`, not this tick's `now`: `now.elapsed()` would
                    // stamp only the microseconds this tick's fetch took.
                    let at_ms = now.duration_since(started).as_millis() as u64;
                    for sample in snapshot_into_samples(snapshot, cpu_percent, at_ms) {
                        producer.push(sample);
                    }
                }
                // else: `MetricsError::Unsupported` (non-Linux desktop
                // source) — every tick will fail identically, but tearing
                // the thread down here would leave `stop()`/Drop with
                // nothing to signal; skipping the tick and trying again next
                // interval is simpler than adding a second shutdown path.

                if wait_or_stop(interval, &stop_flag_worker) {
                    break;
                }
            }
            producer.close();
        });

        SamplerHandle {
            samples: MetricsReceiver { shared },
            stop_flag,
            worker: Some(worker),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};
    use std::sync::Arc as StdArc;

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

    /// A fake Android source with every probe registered, exercised as the
    /// "fake source" the lifecycle test drives — see the crate module doc's
    /// note that `FakeProcessRunner` is the sanctioned seam for exactly this.
    fn fake_android_source() -> MetricsSource {
        let runner = FakeProcessRunner::new()
            .with("adb -s emulator-5554 shell cat /proc/4021/stat", ok(STAT_LINE))
            .with(
                "adb -s emulator-5554 shell dumpsys meminfo it.f0x.frustbench",
                ok(MEMINFO_EXCERPT),
            )
            .with(
                "adb -s emulator-5554 shell for z in /sys/class/thermal/thermal_zone*; do t=$(cat \"$z/type\" 2>/dev/null); v=$(cat \"$z/temp\" 2>/dev/null); echo \"$z|$t|$v\"; done",
                ok("/sys/class/thermal/thermal_zone0|cpu-0|41000\n"),
            )
            .with("adb -s emulator-5554 shell cat /proc/net/dev", ok(NET_DEV));
        MetricsSource::AndroidPkg {
            runner: StdArc::new(runner),
            serial: "emulator-5554".to_string(),
            pid: "4021".to_string(),
            pkg: "it.f0x.frustbench".to_string(),
        }
    }

    #[test]
    fn spawn_delivers_samples_from_a_fake_source() {
        let mut handle =
            MetricsSampler::spawn(fake_android_source(), MetricsSampler::MIN_INTERVAL, 64);

        // First tick fires immediately: mem/thermal/net arrive without a
        // prior reading (cpu needs a second tick to diff against).
        let first = handle.samples.recv().unwrap();
        assert!(matches!(first, MetricsSample::Mem(_)));
        let second = handle.samples.recv().unwrap();
        assert!(matches!(second, MetricsSample::Thermal(_)));
        let third = handle.samples.recv().unwrap();
        assert!(matches!(third, MetricsSample::Net(_)));

        // The second tick (after MIN_INTERVAL) adds a CPU sample now that
        // there's a prior reading to diff against.
        let fourth = handle.samples.recv().unwrap();
        assert!(
            matches!(fourth, MetricsSample::Cpu(_)),
            "expected a Cpu sample once a second tick had a prior reading, got {fourth:?}"
        );

        handle.stop();
    }

    #[test]
    fn stop_terminates_the_background_thread_promptly() {
        let mut handle =
            MetricsSampler::spawn(fake_android_source(), MetricsSampler::MIN_INTERVAL, 64);
        // Let at least one tick land before stopping.
        let _ = handle.samples.recv();

        let start = Instant::now();
        handle.stop();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "stop() should terminate within a couple of poll slices, took {:?}",
            start.elapsed()
        );

        // Idempotent: a second stop() must not panic or hang.
        handle.stop();
    }

    #[test]
    fn drop_without_stop_does_not_block_the_caller() {
        let handle = MetricsSampler::spawn(fake_android_source(), MetricsSampler::MIN_INTERVAL, 64);
        drop(handle);
    }

    #[test]
    fn interval_is_clamped_to_the_minimum() {
        // A near-zero requested interval must not busy-loop `adb` — clamped
        // to MIN_INTERVAL. Proven indirectly: two ticks (four+ samples given
        // the fake source's four probes) must take at least one MIN_INTERVAL,
        // not near-zero wall time.
        let start = Instant::now();
        let mut handle = MetricsSampler::spawn(fake_android_source(), Duration::from_millis(1), 64);
        // Drain the first tick's samples (3, no CPU yet) plus the first
        // sample of the second tick, which cannot arrive before one clamped
        // interval has elapsed.
        for _ in 0..4 {
            let _ = handle.samples.recv();
        }
        assert!(
            start.elapsed() >= MetricsSampler::MIN_INTERVAL,
            "a near-zero interval must be clamped to MIN_INTERVAL, only took {:?}",
            start.elapsed()
        );
        handle.stop();
    }

    #[test]
    fn drop_oldest_under_a_slow_consumer() {
        // A tiny capacity forces drop-oldest almost immediately: each tick
        // pushes 3-4 samples, so a capacity of 2 overflows on the very first
        // tick if the consumer never drains.
        let mut handle =
            MetricsSampler::spawn(fake_android_source(), MetricsSampler::MIN_INTERVAL, 2);

        // Let a few ticks land without ever draining.
        thread::sleep(MetricsSampler::MIN_INTERVAL * 3);
        handle.stop();

        assert!(
            handle.dropped_samples() > 0,
            "a slow/absent consumer under a small capacity must drop-oldest, not block the producer"
        );

        // The buffer itself never exceeds its capacity even after the
        // producer is done.
        let remaining: Vec<_> = handle.samples.iter().collect();
        assert!(
            remaining.len() <= 2,
            "got {} buffered samples",
            remaining.len()
        );
    }

    #[test]
    fn sample_timestamps_advance_with_the_sampler_epoch_not_per_tick() {
        // Regression: `at_ms` must be measured from the sampler's start, not
        // from the top of the current tick (which would pin every sample to
        // ~0 ms no matter how long the sampler has run).
        let mut handle =
            MetricsSampler::spawn(fake_android_source(), MetricsSampler::MIN_INTERVAL, 64);

        fn at_ms(sample: &MetricsSample) -> u64 {
            match sample {
                MetricsSample::Cpu(s) => s.at_ms,
                MetricsSample::Mem(s) => s.at_ms,
                MetricsSample::Thermal(s) => s.at_ms,
                MetricsSample::Net(s) => s.at_ms,
            }
        }

        // First tick: mem/thermal/net.
        let first_tick = handle.samples.recv().unwrap();
        let _ = handle.samples.recv().unwrap();
        let _ = handle.samples.recv().unwrap();
        // Second tick's first sample (cpu joins once a prior reading exists).
        let second_tick = handle.samples.recv().unwrap();
        handle.stop();

        let (a, b) = (at_ms(&first_tick), at_ms(&second_tick));
        assert!(
            b > a,
            "second tick's at_ms ({b}) must exceed the first's ({a})"
        );
        assert!(
            b >= MetricsSampler::MIN_INTERVAL.as_millis() as u64,
            "a second-tick sample must carry at least one interval of epoch time, got {b} ms"
        );
    }
}
