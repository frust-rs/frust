//! Desktop Linux metrics: pure parsers over `/proc/<pid>/stat`,
//! `/proc/<pid>/status`'s `VmRSS`, `/sys/class/thermal/thermal_zone*`, and
//! `/proc/net/dev`, plus a thin `std::fs` reader layer feeding them. No new
//! dependency (no `sysinfo`, no `libc`) — a deliberate choice over a
//! `sysinfo`-based design: these four sources
//! are simple enough to parse directly and `frust-drive` already has a hard
//! zero-new-dependency bar for this feature.
//!
//! **Linux-only.** macOS/Windows builds still compile (see the crate-level
//! `#[cfg]` split below) but every fetch reports
//! [`MetricsError::Unsupported`] at runtime rather than failing to build —
//! the macOS gap is tracked as a `docs/LIMITATIONS.md` entry, not silently
//! absent.

use std::path::Path;

use super::SnapshotRaw;
use super::net_dev::{aggregate_excluding_loopback, parse_proc_net_dev};
use super::proc_stat::{CpuTicks, parse_proc_stat_ticks};
use super::{MetricsError, ThermalReading};

/// Real `/proc` root — overridden in tests via the `*_at` variants below so
/// the reader layer is testable without touching the real filesystem.
#[cfg(target_os = "linux")]
const PROC_ROOT: &str = "/proc";
/// Real `/sys/class/thermal` root — same testability reasoning.
#[cfg(target_os = "linux")]
const THERMAL_ROOT: &str = "/sys/class/thermal";

#[cfg(target_os = "linux")]
pub(super) fn fetch_all(pid: u32) -> Result<SnapshotRaw, MetricsError> {
    Ok(SnapshotRaw {
        cpu_ticks: read_cpu_ticks_at(Path::new(PROC_ROOT), pid).ok(),
        mem_rss_bytes: read_vmrss_at(Path::new(PROC_ROOT), pid).ok(),
        thermal: read_thermal_zones_at(Path::new(THERMAL_ROOT)),
        net: read_net_dev_at(Path::new(PROC_ROOT)).ok(),
    })
}

#[cfg(not(target_os = "linux"))]
pub(super) fn fetch_all(_pid: u32) -> Result<SnapshotRaw, MetricsError> {
    Err(MetricsError::Unsupported(std::env::consts::OS))
}

/// Reads `<proc_root>/<pid>/stat` and parses its `utime`/`stime` fields.
/// Portable itself (plain `std::fs` against a caller-given root) — only
/// [`fetch_all`]'s Linux branch feeds it the real `/proc`; the other targets'
/// `fetch_all` never calls it, but tests exercise it directly against a temp
/// dir on every host.
fn read_cpu_ticks_at(proc_root: &Path, pid: u32) -> Result<CpuTicks, MetricsError> {
    let path = proc_root.join(pid.to_string()).join("stat");
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| MetricsError::io(path.display().to_string(), e))?;
    parse_proc_stat_ticks(&contents)
}

/// Reads `<proc_root>/<pid>/status` and parses its `VmRSS` line, in bytes
/// (the file reports kibibytes).
fn read_vmrss_at(proc_root: &Path, pid: u32) -> Result<u64, MetricsError> {
    let path = proc_root.join(pid.to_string()).join("status");
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| MetricsError::io(path.display().to_string(), e))?;
    parse_vmrss_kb(&contents).map(|kb| kb * 1024)
}

/// Pure parse of `/proc/<pid>/status`'s `VmRSS:` line, in kibibytes.
fn parse_vmrss_kb(contents: &str) -> Result<u64, MetricsError> {
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb = rest.split_whitespace().next().ok_or_else(|| {
                MetricsError::parse("proc/pid/status VmRSS", "no value after `VmRSS:`")
            })?;
            return kb
                .parse::<u64>()
                .map_err(|e| MetricsError::parse("proc/pid/status VmRSS", format!("`{kb}`: {e}")));
        }
    }
    Err(MetricsError::parse(
        "proc/pid/status VmRSS",
        "no `VmRSS:` line found",
    ))
}

/// Walks `<root>/thermal_zone*`, reading each zone's `type`+`temp`.
/// Never hard-fails: a directory that doesn't exist (no thermal sensors —
/// common in VMs/containers) yields an empty `Vec`, and a single zone whose
/// `temp` can't be read (permission denied, zone removed mid-scan) is
/// skipped rather than aborting the whole walk — the same "one bad field
/// doesn't fail the tick" contract every fetcher in this crate follows.
fn read_thermal_zones_at(root: &Path) -> Vec<ThermalReading> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    let mut zone_dirs: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("thermal_zone"))
        })
        .collect();
    zone_dirs.sort();

    for zone_dir in zone_dirs {
        let Ok(temp_contents) = std::fs::read_to_string(zone_dir.join("temp")) else {
            continue;
        };
        let type_contents = std::fs::read_to_string(zone_dir.join("type")).unwrap_or_default();
        let fallback_label = zone_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("thermal_zone")
            .to_string();
        if let Ok(reading) = parse_thermal_zone(&fallback_label, &type_contents, &temp_contents) {
            out.push(reading);
        }
    }
    out
}

/// Pure parse of one thermal zone's already-read `type`/`temp` file content.
/// `fallback_label` (the zone's directory name, e.g. `thermal_zone0`) is used
/// when `type_contents` is empty/unreadable.
fn parse_thermal_zone(
    fallback_label: &str,
    type_contents: &str,
    temp_contents: &str,
) -> Result<ThermalReading, MetricsError> {
    let millideg_c: i64 = temp_contents.trim().parse().map_err(|_| {
        MetricsError::parse(
            "sys/class/thermal temp",
            format!("not an integer: {:?}", temp_contents.trim()),
        )
    })?;
    let label = type_contents.trim();
    let zone_label = if label.is_empty() {
        fallback_label.to_string()
    } else {
        label.to_string()
    };
    Ok(ThermalReading {
        zone_label,
        millideg_c,
    })
}

/// Reads `<proc_root>/net/dev` and aggregates every non-loopback interface —
/// namespace-wide, see the crate module doc's scope note.
fn read_net_dev_at(proc_root: &Path) -> Result<(u64, u64), MetricsError> {
    let path = proc_root.join("net/dev");
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| MetricsError::io(path.display().to_string(), e))?;
    let ifaces = parse_proc_net_dev(&contents)?;
    Ok(aggregate_excluding_loopback(&ifaces))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vmrss_kb_reads_the_vmrss_line() {
        let status = "\
Name:   frustbench
State:  S (sleeping)
VmPeak:   123456 kB
VmRSS:     45678 kB
VmData:    98765 kB
";
        assert_eq!(parse_vmrss_kb(status).unwrap(), 45678);
    }

    #[test]
    fn parse_vmrss_kb_missing_line_errs() {
        assert!(parse_vmrss_kb("Name: frustbench\nState: S\n").is_err());
    }

    #[test]
    fn parse_thermal_zone_uses_type_when_present() {
        let reading = parse_thermal_zone("thermal_zone0", "cpu-0\n", "41000\n").unwrap();
        assert_eq!(
            reading,
            ThermalReading {
                zone_label: "cpu-0".to_string(),
                millideg_c: 41000
            }
        );
    }

    #[test]
    fn parse_thermal_zone_falls_back_to_directory_name_when_type_is_empty() {
        let reading = parse_thermal_zone("thermal_zone3", "", "39500\n").unwrap();
        assert_eq!(
            reading,
            ThermalReading {
                zone_label: "thermal_zone3".to_string(),
                millideg_c: 39500
            }
        );
    }

    #[test]
    fn parse_thermal_zone_non_integer_temp_errs() {
        assert!(parse_thermal_zone("thermal_zone0", "cpu-0", "not-a-number").is_err());
    }

    /// Reader-level fixture: a temp dir shaped like `/sys/class/thermal` with
    /// two readable zones plus one whose `temp` file is simply absent
    /// (portable stand-in for "permission denied" — chmod-based EACCES tests
    /// are unreliable when the test runner is root, which CI sometimes is).
    /// Proves `read_thermal_zones_at` skips the unreadable zone but keeps the
    /// other two, matching [`super::android`]'s permission-denied fixture.
    #[test]
    fn read_thermal_zones_at_skips_a_zone_whose_temp_is_unreadable() {
        let tmp = std::env::temp_dir().join(format!(
            "frust-metrics-thermal-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let zone0 = tmp.join("thermal_zone0");
        let zone1 = tmp.join("thermal_zone1");
        let zone2 = tmp.join("thermal_zone2");
        std::fs::create_dir_all(&zone0).unwrap();
        std::fs::create_dir_all(&zone1).unwrap();
        std::fs::create_dir_all(&zone2).unwrap();
        std::fs::write(zone0.join("type"), "cpu-0\n").unwrap();
        std::fs::write(zone0.join("temp"), "41000\n").unwrap();
        std::fs::write(zone1.join("type"), "cpu-1\n").unwrap();
        std::fs::write(zone1.join("temp"), "39500\n").unwrap();
        std::fs::write(zone2.join("type"), "battery\n").unwrap();
        // zone2's `temp` file is deliberately never written — simulates an
        // unreadable/permission-denied zone.

        let readings = read_thermal_zones_at(&tmp);
        assert_eq!(
            readings,
            vec![
                ThermalReading {
                    zone_label: "cpu-0".to_string(),
                    millideg_c: 41000
                },
                ThermalReading {
                    zone_label: "cpu-1".to_string(),
                    millideg_c: 39500
                },
            ]
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_thermal_zones_at_missing_root_yields_empty_not_error() {
        let missing = std::env::temp_dir().join("frust-metrics-thermal-does-not-exist");
        assert_eq!(read_thermal_zones_at(&missing), Vec::new());
    }

    /// Reader-level fixture proving `read_cpu_ticks_at`/`read_vmrss_at`
    /// correctly join `<root>/<pid>/...` and hand off to the pure parsers.
    #[test]
    fn read_cpu_ticks_and_vmrss_at_join_pid_directory_correctly() {
        let tmp = std::env::temp_dir().join(format!(
            "frust-metrics-proc-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pid_dir = tmp.join("4021");
        std::fs::create_dir_all(&pid_dir).unwrap();
        std::fs::write(
            pid_dir.join("stat"),
            "4021 (frustbench) S 1 4021 4021 0 -1 4194560 227 0 0 0 42 17 0 0 20 0 4 0 12345678 123456789 1234 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0 0 0 0 0 0 0 0 0",
        )
        .unwrap();
        std::fs::write(
            pid_dir.join("status"),
            "Name: frustbench\nVmRSS:  45678 kB\n",
        )
        .unwrap();

        let ticks = read_cpu_ticks_at(&tmp, 4021).unwrap();
        assert_eq!(
            ticks,
            CpuTicks {
                utime: 42,
                stime: 17
            }
        );
        assert_eq!(read_vmrss_at(&tmp, 4021).unwrap(), 45678 * 1024);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_cpu_ticks_at_missing_pid_errs() {
        let missing = std::env::temp_dir().join("frust-metrics-proc-missing-pid");
        assert!(read_cpu_ticks_at(&missing, 999999).is_err());
    }

    #[test]
    fn read_net_dev_at_reads_and_aggregates() {
        let tmp = std::env::temp_dir().join(format!(
            "frust-metrics-netdev-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let net_dir = tmp.join("net");
        std::fs::create_dir_all(&net_dir).unwrap();
        std::fs::write(
            net_dir.join("dev"),
            "Inter-|   Receive                                                |  Transmit\n \
             face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
             lo: 100 0 0 0 0 0 0 0 100 0 0 0 0 0 0 0\n\
             eth0: 5000 0 0 0 0 0 0 0 3000 0 0 0 0 0 0 0\n",
        )
        .unwrap();

        assert_eq!(read_net_dev_at(&tmp).unwrap(), (5000, 3000));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
