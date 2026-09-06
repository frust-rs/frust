//! Per-session system-metrics sampling: one thread that owns a
//! `frust_drive::metrics::SamplerHandle` and folds each tick into the
//! session's latest-per-kind record.
//!
//! # Android only, and why
//!
//! Sampling needs the app's pid. `frust_drive::process::StreamHandle` — the
//! seam every desktop/iOS session is spawned through — does not expose the
//! child's pid, and widening `frust-drive` is out of scope here, so **only an
//! Android session can sample**: it recovers `(serial, pid, package)` from
//! the drive's own log markers (see [`super::launch`]). A desktop or iOS
//! session reports `metrics_sampling: false` with empty samples forever, and
//! the tool layer must render that as "metrics unavailable on this target"
//! rather than as a zeroed reading. `frust-tui` carries the identical gap
//! for the identical reason (`MetricsIdentity::NotAndroid`).
//!
//! # Teardown: signal, never join the sampler
//!
//! The sampler's own thread makes `adb` calls, and `frust-drive` puts no
//! wall-clock bound on a `Command`, so one wedged `adb shell` against an
//! unresponsive device parks it indefinitely. `SamplerHandle::stop` *joins*
//! that thread; this module therefore never calls it — the handle is simply
//! dropped, whose `Drop` signals and detaches. This drain thread itself polls
//! in [`POLL_SLICE`] steps (`MetricsReceiver` has no `recv_timeout`), so it
//! always winds down within one slice and is safe for teardown to join.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use frust_drive::metrics::{MetricsSampler, MetricsSource, MetricsTryRecvError};

use super::Runner;
use super::session::Session;

/// The sampler's ring capacity — generous against [`POLL_SLICE`], so a slow
/// drain tick never loses a sample before this thread gets to it.
const SAMPLER_CAPACITY: usize = 64;

/// How finely the drain loop polls for a new sample and for the stop flag.
const POLL_SLICE: Duration = Duration::from_millis(20);

/// Starts sampling `(serial, pid, package)` for `session`. A no-op once
/// teardown has begun. The caller has already marked the session as sampling
/// (`Session::note_pid`/`note_android_package` hand out the identity exactly
/// once), so this never starts a second sampler for one session.
pub(crate) fn spawn_sampler(
    session: &Arc<Session>,
    runner: &Runner,
    serial: String,
    pid: String,
    package: String,
) {
    if session.stopping() {
        session.clear_metrics_sampling();
        return;
    }
    let source = MetricsSource::AndroidPkg {
        runner: Arc::clone(runner),
        serial,
        pid,
        pkg: package,
    };
    let thread_session = Arc::clone(session);
    let handle = thread::spawn(move || run(thread_session, source));
    session.add_thread(handle);
}

fn run(session: Arc<Session>, source: MetricsSource) {
    let handle = MetricsSampler::spawn(source, MetricsSampler::DEFAULT_INTERVAL, SAMPLER_CAPACITY);
    while !session.stopping() {
        match handle.samples.try_recv() {
            Ok(sample) => session.record_metric(sample),
            Err(MetricsTryRecvError::Empty) => thread::sleep(POLL_SLICE),
            Err(MetricsTryRecvError::Disconnected) => break,
        }
    }
    session.clear_metrics_sampling();
    // `handle` drops here — signal-and-detach, never `stop()`. See the
    // module doc.
}
