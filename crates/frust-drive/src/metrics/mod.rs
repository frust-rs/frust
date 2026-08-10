//! Streamable system-metrics sampling for the TUI DevTools System/Network
//! tabs: per-process CPU%/RSS + thermal on desktop Linux, read directly from
//! `/proc`/`/sys` (no dependency — see [`desktop`]), and the Android
//! equivalent driven through `adb` via the crate's `&dyn ProcessRunner` seam
//! (see [`android`]). [`MetricsSampler::spawn`] turns either source into a
//! background-thread stream of [`MetricsSample`]s.
//!
//! **Honest scope, stated once here:**
//! - Desktop network counters (`/proc/net/dev`) are **namespace-wide** — every
//!   process sharing this network namespace sees the identical counters —
//!   never per-process. `/proc/<pid>/net/dev` exists but reads the same
//!   namespace-wide table for any pid that hasn't been given its own netns.
//! - Android network counters (`adb shell cat /proc/net/dev`) are
//!   **device-wide** for the same reason, one level further removed (the
//!   whole device, not just this app's process).
//! - Desktop metrics are **Linux-only** in this version; macOS/Windows
//!   collectors return [`MetricsError::Unsupported`] rather than failing to
//!   compile — see `docs/LIMITATIONS.md` for the macOS gap this leaves open.
//! - GPU metrics are out of scope entirely (root-only paths on both
//!   platforms) — not attempted here.
//!
//! Every parser in [`proc_stat`]/[`net_dev`]/[`desktop`]/[`android`] is pure
//! (`&str` in, a sample/error out); the filesystem/`adb` reads that feed them
//! are a thin, separately-testable layer, mirroring `process.rs`'s
//! parse-vs-spawn split.

mod android;
mod desktop;
mod net_dev;
mod proc_stat;
mod sampler;

use std::sync::Arc;

use crate::process::ProcessRunner;

pub use proc_stat::CpuTicks;
pub use sampler::{
    MetricsReceiver, MetricsRecvError, MetricsSampler, MetricsTryRecvError, SamplerHandle,
};

/// Milliseconds elapsed since the owning [`MetricsSampler`] was spawned —
/// every sample carries one so a consumer can plot/correlate without
/// depending on delivery order.
pub type TimestampMs = u64;

/// Per-process CPU utilization since the previous tick, as a percentage of
/// one core (100.0 == one core fully busy) — delta-based from `utime`+`stime`
/// jiffies vs. wall-clock elapsed (see [`proc_stat::cpu_percent_from_ticks`]).
/// The first tick after a sampler starts (or after a probe momentarily fails)
/// has no prior reading to diff against, so it emits no [`CpuSample`] rather
/// than a fabricated `0.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuSample {
    pub percent: f32,
    pub at_ms: TimestampMs,
}

/// Per-process resident set size, in bytes (`VmRSS` on desktop, `TOTAL PSS`
/// on Android — see the module doc's PSS-vs-RSS note in [`android`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemSample {
    pub rss_bytes: u64,
    pub at_ms: TimestampMs,
}

/// One thermal zone's reading. `zone_label` is the zone's `type` file content
/// (e.g. `cpu-0`, `x86_pkg_temp`) falling back to the zone's directory name
/// when `type` is unreadable/empty. `millideg_c` is the raw value `/sys`
/// reports — Linux desktop zones are millidegrees Celsius by kernel contract;
/// Android vendors are inconsistent about whether `temp` is already
/// millidegrees or plain degrees, so this is reported **as the device
/// reports it**, unconverted — a consumer comparing zones across devices
/// must not assume a shared scale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThermalSample {
    pub zone_label: String,
    pub millideg_c: i64,
    pub at_ms: TimestampMs,
}

/// Cumulative network byte counters as read this tick (never diffed here —
/// see the module doc's namespace-wide/device-wide scope note). A consumer
/// wanting a rate diffs two ticks' counters over their `at_ms` delta itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetSample {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub at_ms: TimestampMs,
}

/// One sampled value, tagged by kind — what [`MetricsSampler::spawn`]'s
/// [`SamplerHandle::samples`] stream carries. A single tick can produce
/// several (CPU, mem, one per thermal zone, net), each pushed as its own
/// entry rather than a bundled struct, so a consumer that only cares about
/// one kind can filter cheaply.
#[derive(Debug, Clone, PartialEq)]
pub enum MetricsSample {
    Cpu(CpuSample),
    Mem(MemSample),
    Thermal(ThermalSample),
    Net(NetSample),
}

/// Errors a metrics parser/fetcher can report. Never fatal to a running
/// [`MetricsSampler`] — a single failed probe within a tick is dropped (see
/// `sampler::fetch_raw`'s callers), so a transient `adb` hiccup or an exited
/// process skips one field for one tick rather than tearing down the stream.
#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    /// The desktop collector needs Linux's `/proc`+`/sys`; this OS has no
    /// equivalent wired up (documented gap, not a compile-time cfg failure —
    /// see the module doc).
    #[error(
        "desktop process/thermal/network metrics need Linux (/proc, /sys); unsupported on `{0}`"
    )]
    Unsupported(&'static str),
    /// A parser rejected its input — the exact source string is not included
    /// (may be large/binary-ish); `reason` names what was wrong.
    #[error("failed to parse {source_desc}: {reason}")]
    Parse {
        source_desc: &'static str,
        reason: String,
    },
    /// A `std::fs` read failed (missing file, permission denied, process
    /// exited between enumeration and read).
    #[error("reading `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// An `adb` invocation failed to spawn, or exited non-zero.
    #[error("adb command failed: {0}")]
    Adb(String),
}

impl MetricsError {
    pub(crate) fn parse(source_desc: &'static str, reason: impl Into<String>) -> Self {
        Self::Parse {
            source_desc,
            reason: reason.into(),
        }
    }

    pub(crate) fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// One thermal zone's raw reading, shared shape between [`desktop`]'s
/// `/sys/class/thermal` walk and [`android`]'s `adb`-driven equivalent,
/// before either attaches a sample timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThermalReading {
    pub(crate) zone_label: String,
    pub(crate) millideg_c: i64,
}

/// Where a [`MetricsSampler`] pulls a tick's readings from.
pub enum MetricsSource {
    /// Desktop Linux: sample process `pid` via `/proc`+`/sys` directly.
    DesktopPid(u32),
    /// Android: sample `pkg` (installed on device `serial`, running as
    /// `pid`) via `adb`, through the injected runner — the crate's hard rule
    /// that every external tool invocation goes through `&dyn ProcessRunner`
    /// applies here too, so this is exercised in tests via
    /// `FakeProcessRunner` and never shells out for real during `cargo test`.
    AndroidPkg {
        runner: Arc<dyn ProcessRunner + Send + Sync>,
        serial: String,
        pid: String,
        pkg: String,
    },
}

/// One tick's raw, not-yet-timestamped readings — each field independently
/// optional/best-effort (see [`MetricsError`]'s doc: one failed probe never
/// fails the whole tick).
pub(crate) struct SnapshotRaw {
    pub(crate) cpu_ticks: Option<proc_stat::CpuTicks>,
    pub(crate) mem_rss_bytes: Option<u64>,
    pub(crate) thermal: Vec<ThermalReading>,
    pub(crate) net: Option<(u64, u64)>,
}

/// Fetches one tick's raw readings from `source`. Never returns `Err` except
/// for [`MetricsError::Unsupported`] (a non-Linux desktop source) — every
/// other per-field failure is folded into that field being absent, per
/// [`SnapshotRaw`]'s doc.
pub(crate) fn fetch_raw(source: &MetricsSource) -> Result<SnapshotRaw, MetricsError> {
    match source {
        MetricsSource::DesktopPid(pid) => desktop::fetch_all(*pid),
        MetricsSource::AndroidPkg {
            runner,
            serial,
            pid,
            pkg,
        } => Ok(android::fetch_all(runner.as_ref(), serial, pid, pkg)),
    }
}

/// Converts one tick's [`SnapshotRaw`] into the [`MetricsSample`]s it yields,
/// stamping every one with `at_ms`. `cpu_percent` is computed by the caller
/// (the sampler thread, which alone holds the previous tick's ticks to diff
/// against — see [`proc_stat::cpu_percent_from_ticks`]) since a bare
/// snapshot has no history of its own.
pub(crate) fn snapshot_into_samples(
    snapshot: SnapshotRaw,
    cpu_percent: Option<f32>,
    at_ms: TimestampMs,
) -> Vec<MetricsSample> {
    let mut out = Vec::new();
    if let Some(percent) = cpu_percent {
        out.push(MetricsSample::Cpu(CpuSample { percent, at_ms }));
    }
    if let Some(rss_bytes) = snapshot.mem_rss_bytes {
        out.push(MetricsSample::Mem(MemSample { rss_bytes, at_ms }));
    }
    for reading in snapshot.thermal {
        out.push(MetricsSample::Thermal(ThermalSample {
            zone_label: reading.zone_label,
            millideg_c: reading.millideg_c,
            at_ms,
        }));
    }
    if let Some((rx_bytes, tx_bytes)) = snapshot.net {
        out.push(MetricsSample::Net(NetSample {
            rx_bytes,
            tx_bytes,
            at_ms,
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_into_samples_emits_cpu_only_when_percent_present() {
        let snapshot = SnapshotRaw {
            cpu_ticks: None,
            mem_rss_bytes: None,
            thermal: Vec::new(),
            net: None,
        };
        assert!(snapshot_into_samples(snapshot, None, 0).is_empty());

        let snapshot = SnapshotRaw {
            cpu_ticks: None,
            mem_rss_bytes: None,
            thermal: Vec::new(),
            net: None,
        };
        let samples = snapshot_into_samples(snapshot, Some(12.5), 100);
        assert_eq!(
            samples,
            vec![MetricsSample::Cpu(CpuSample {
                percent: 12.5,
                at_ms: 100
            })]
        );
    }

    #[test]
    fn snapshot_into_samples_emits_one_thermal_sample_per_zone() {
        let snapshot = SnapshotRaw {
            cpu_ticks: None,
            mem_rss_bytes: Some(2048),
            thermal: vec![
                ThermalReading {
                    zone_label: "cpu-0".to_string(),
                    millideg_c: 41000,
                },
                ThermalReading {
                    zone_label: "cpu-1".to_string(),
                    millideg_c: 39500,
                },
            ],
            net: Some((10, 20)),
        };
        let samples = snapshot_into_samples(snapshot, None, 250);
        assert_eq!(
            samples,
            vec![
                MetricsSample::Mem(MemSample {
                    rss_bytes: 2048,
                    at_ms: 250
                }),
                MetricsSample::Thermal(ThermalSample {
                    zone_label: "cpu-0".to_string(),
                    millideg_c: 41000,
                    at_ms: 250
                }),
                MetricsSample::Thermal(ThermalSample {
                    zone_label: "cpu-1".to_string(),
                    millideg_c: 39500,
                    at_ms: 250
                }),
                MetricsSample::Net(NetSample {
                    rx_bytes: 10,
                    tx_bytes: 20,
                    at_ms: 250
                }),
            ]
        );
    }
}
