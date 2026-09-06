//! Android metrics: pure parsers plus `adb`-driven fetchers, every fetcher
//! routed through `&dyn ProcessRunner` (the crate's hard rule — see
//! `process.rs`), so this whole module is testable via `FakeProcessRunner`
//! and never shells out for real during `cargo test`.
//!
//! **Memory is PSS, not RSS.** Android's `dumpsys meminfo`'s `TOTAL PSS` is
//! proportional set size — shared pages counted fractionally across the
//! processes mapping them — not resident set size; it is what Android's own
//! tooling (and this crate's benchmarks harness, `benchmarks/PROTOCOL.md`
//! §7's idle-memory note) uses for cross-run app memory comparisons, so
//! [`MemSample`](super::MemSample) reports it as-is on Android rather than
//! forcing an RSS-shaped number that isn't what `dumpsys` actually gives.
//!
//! **Network is device-wide**, exactly like desktop's namespace-wide
//! `/proc/net/dev` read — see the crate module doc.

use crate::process::ProcessRunner;

use super::net_dev::{aggregate_excluding_loopback, parse_proc_net_dev};
use super::proc_stat::{CpuTicks, parse_proc_stat_ticks};
use super::{MetricsError, SnapshotRaw, ThermalReading};

/// Best-effort snapshot of everything this module can fetch for `pkg`
/// (running as `pid`) on device `serial`. Each field is independently
/// optional — one failed `adb` round-trip (device unplugged mid-tick, a
/// permission-denied thermal zone) never fails the whole tick, matching
/// every other fetcher in this crate.
pub(super) fn fetch_all(
    runner: &dyn ProcessRunner,
    serial: &str,
    pid: &str,
    pkg: &str,
) -> SnapshotRaw {
    SnapshotRaw {
        cpu_ticks: fetch_cpu_ticks(runner, serial, pid).ok(),
        mem_rss_bytes: fetch_mem_pss_bytes(runner, serial, pkg).ok(),
        thermal: fetch_thermal(runner, serial).unwrap_or_default(),
        net: fetch_net(runner, serial).ok(),
    }
}

/// `adb -s <serial> shell cat <remote_path>`, erroring on a spawn failure or
/// a non-zero exit (a missing file, no permission).
fn adb_shell_cat(
    runner: &dyn ProcessRunner,
    serial: &str,
    remote_path: &str,
) -> Result<String, MetricsError> {
    let out = runner
        .run("adb", &["-s", serial, "shell", "cat", remote_path])
        .map_err(|e| MetricsError::Adb(e.to_string()))?;
    if !out.success {
        return Err(MetricsError::Adb(format!(
            "adb -s {serial} shell cat {remote_path}: {}",
            out.stderr.trim()
        )));
    }
    Ok(out.stdout)
}

/// `adb -s <serial> shell cat /proc/<pid>/stat`, parsed by the same pure
/// parser [`super::desktop`] uses — the file's shape is identical on-device.
pub(super) fn fetch_cpu_ticks(
    runner: &dyn ProcessRunner,
    serial: &str,
    pid: &str,
) -> Result<CpuTicks, MetricsError> {
    let remote_path = format!("/proc/{pid}/stat");
    let contents = adb_shell_cat(runner, serial, &remote_path)?;
    parse_proc_stat_ticks(&contents)
}

/// `adb -s <serial> shell dumpsys meminfo <pkg>`, parsed for the `TOTAL PSS`
/// row, in bytes (the row reports kibibytes) — see the module doc's
/// PSS-not-RSS note.
pub(super) fn fetch_mem_pss_bytes(
    runner: &dyn ProcessRunner,
    serial: &str,
    pkg: &str,
) -> Result<u64, MetricsError> {
    let out = runner
        .run("adb", &["-s", serial, "shell", "dumpsys", "meminfo", pkg])
        .map_err(|e| MetricsError::Adb(e.to_string()))?;
    if !out.success {
        return Err(MetricsError::Adb(format!(
            "adb -s {serial} shell dumpsys meminfo {pkg}: {}",
            out.stderr.trim()
        )));
    }
    let kb = parse_meminfo_total_pss_kb(&out.stdout)?;
    Ok(kb * 1024)
}

/// Pure parse of `dumpsys meminfo`'s `App Summary` section for its
/// `TOTAL PSS:` row (e.g. `           TOTAL PSS:    90277            TOTAL
/// RSS:   190372       TOTAL SWAP PSS:      253`), returning the PSS value in
/// kibibytes — the first whitespace-separated token after `TOTAL PSS:`.
fn parse_meminfo_total_pss_kb(contents: &str) -> Result<u64, MetricsError> {
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("TOTAL PSS:") {
            let value = rest.split_whitespace().next().ok_or_else(|| {
                MetricsError::parse("dumpsys meminfo TOTAL PSS", "no value after `TOTAL PSS:`")
            })?;
            return value.parse::<u64>().map_err(|e| {
                MetricsError::parse("dumpsys meminfo TOTAL PSS", format!("`{value}`: {e}"))
            });
        }
    }
    Err(MetricsError::parse(
        "dumpsys meminfo TOTAL PSS",
        "no `TOTAL PSS:` row found",
    ))
}

/// One shell invocation enumerating every thermal zone's `type`+`temp`,
/// pipe-delimited per line (`<zone-path>|<type>|<temp>`), redirecting each
/// individual `cat`'s stderr to `/dev/null` rather than letting one
/// unreadable (root-required) zone abort the whole loop —
/// [`parse_thermal_shell_output`] then skips any line whose `temp` field
/// didn't parse as an integer, which is exactly what a permission-denied
/// zone produces (an empty field, since `2>/dev/null` swallows the error
/// text `cat` would otherwise have written).
const THERMAL_SHELL_SCRIPT: &str = "for z in /sys/class/thermal/thermal_zone*; do t=$(cat \"$z/type\" 2>/dev/null); v=$(cat \"$z/temp\" 2>/dev/null); echo \"$z|$t|$v\"; done";

/// `adb -s <serial> shell <THERMAL_SHELL_SCRIPT>` — see
/// [`THERMAL_SHELL_SCRIPT`]'s doc for the permission-denied tolerance this
/// buys.
pub(super) fn fetch_thermal(
    runner: &dyn ProcessRunner,
    serial: &str,
) -> Result<Vec<ThermalReading>, MetricsError> {
    let out = runner
        .run("adb", &["-s", serial, "shell", THERMAL_SHELL_SCRIPT])
        .map_err(|e| MetricsError::Adb(e.to_string()))?;
    if !out.success {
        return Err(MetricsError::Adb(format!(
            "adb -s {serial} shell <thermal probe>: {}",
            out.stderr.trim()
        )));
    }
    Ok(parse_thermal_shell_output(&out.stdout))
}

/// Pure parse of [`THERMAL_SHELL_SCRIPT`]'s `<zone-path>|<type>|<temp>` line
/// format. A zone whose `temp` field is empty or non-numeric (the
/// permission-denied case — see the script's doc) is silently skipped, never
/// erroring the whole batch; `zone_label` prefers `type`, falling back to the
/// zone path's basename when `type` is itself empty/unreadable.
fn parse_thermal_shell_output(contents: &str) -> Vec<ThermalReading> {
    let mut out = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let zone_path = parts.next().unwrap_or_default();
        let zone_type = parts.next().unwrap_or_default().trim();
        let temp_raw = parts.next().unwrap_or_default().trim();
        let Ok(millideg_c) = temp_raw.parse::<i64>() else {
            continue;
        };
        let zone_label = if zone_type.is_empty() {
            zone_path
                .rsplit('/')
                .next()
                .unwrap_or(zone_path)
                .to_string()
        } else {
            zone_type.to_string()
        };
        out.push(ThermalReading {
            zone_label,
            millideg_c,
        });
    }
    out
}

/// `adb -s <serial> shell cat /proc/net/dev`, aggregated excluding loopback —
/// device-wide, see the module doc.
pub(super) fn fetch_net(
    runner: &dyn ProcessRunner,
    serial: &str,
) -> Result<(u64, u64), MetricsError> {
    let contents = adb_shell_cat(runner, serial, "/proc/net/dev")?;
    let ifaces = parse_proc_net_dev(&contents)?;
    Ok(aggregate_excluding_loopback(&ifaces))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn failed(stderr: &str) -> Output {
        Output {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    /// Real-captured excerpt (see `benchmarks/raw/*/frust_release/*/run-*.pss_after.txt`)
    /// of `dumpsys meminfo`'s `App Summary` section.
    const DUMPSYS_MEMINFO_EXCERPT: &str = "\
 App Summary
                       Pss(KB)                        Rss(KB)
                        ------                         ------
           Java Heap:     2568                          21124
         Native Heap:    49696                          51032
                Code:    23084                         105504
               Stack:      440                            444
            Graphics:     7176                           7180
       Private Other:     1748
              System:     5565
             Unknown:                                    5088

           TOTAL PSS:    90277            TOTAL RSS:   190372       TOTAL SWAP PSS:      253

 Objects
";

    #[test]
    fn parse_meminfo_total_pss_kb_reads_the_total_pss_row() {
        assert_eq!(
            parse_meminfo_total_pss_kb(DUMPSYS_MEMINFO_EXCERPT).unwrap(),
            90277
        );
    }

    #[test]
    fn parse_meminfo_total_pss_kb_missing_row_errs() {
        assert!(parse_meminfo_total_pss_kb("no such row here\n").is_err());
    }

    #[test]
    fn parse_thermal_shell_output_skips_permission_denied_zone() {
        // zone1's `v` (temp) is empty — the shell script's `2>/dev/null`
        // swallowing a permission-denied `cat`.
        let output = "\
/sys/class/thermal/thermal_zone0|cpu-0|41000
/sys/class/thermal/thermal_zone1||
/sys/class/thermal/thermal_zone2|battery|39500
";
        let readings = parse_thermal_shell_output(output);
        assert_eq!(
            readings,
            vec![
                ThermalReading {
                    zone_label: "cpu-0".to_string(),
                    millideg_c: 41000
                },
                ThermalReading {
                    zone_label: "battery".to_string(),
                    millideg_c: 39500
                },
            ]
        );
    }

    #[test]
    fn parse_thermal_shell_output_falls_back_to_zone_path_when_type_empty() {
        let output = "/sys/class/thermal/thermal_zone7||36000\n";
        let readings = parse_thermal_shell_output(output);
        assert_eq!(
            readings,
            vec![ThermalReading {
                zone_label: "thermal_zone7".to_string(),
                millideg_c: 36000
            }]
        );
    }

    #[test]
    fn parse_thermal_shell_output_empty_content_is_empty() {
        assert_eq!(parse_thermal_shell_output(""), Vec::new());
    }

    #[test]
    fn fetch_cpu_ticks_success() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell cat /proc/4021/stat",
            ok("4021 (frustbench) S 1 4021 4021 0 -1 4194560 227 0 0 0 42 17 0 0 20 0 4 0 12345678 123456789 1234 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0 0 0 0 0 0 0 0 0"),
        );
        let ticks = fetch_cpu_ticks(&runner, "emulator-5554", "4021").unwrap();
        assert_eq!(
            ticks,
            CpuTicks {
                utime: 42,
                stime: 17
            }
        );
    }

    #[test]
    fn fetch_cpu_ticks_garbage_output_errs() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell cat /proc/4021/stat",
            ok("not a stat line\n"),
        );
        assert!(fetch_cpu_ticks(&runner, "emulator-5554", "4021").is_err());
    }

    #[test]
    fn fetch_cpu_ticks_failed_exit_errs() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell cat /proc/4021/stat",
            failed("cat: /proc/4021/stat: No such file or directory"),
        );
        assert!(fetch_cpu_ticks(&runner, "emulator-5554", "4021").is_err());
    }

    #[test]
    fn fetch_mem_pss_bytes_success() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell dumpsys meminfo it.f0x.frustbench",
            ok(DUMPSYS_MEMINFO_EXCERPT),
        );
        assert_eq!(
            fetch_mem_pss_bytes(&runner, "emulator-5554", "it.f0x.frustbench").unwrap(),
            90277 * 1024
        );
    }

    #[test]
    fn fetch_mem_pss_bytes_garbage_output_errs() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell dumpsys meminfo it.f0x.frustbench",
            ok("Unknown package: it.f0x.frustbench\n"),
        );
        assert!(fetch_mem_pss_bytes(&runner, "emulator-5554", "it.f0x.frustbench").is_err());
    }

    #[test]
    fn fetch_mem_pss_bytes_failed_exit_errs() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell dumpsys meminfo it.f0x.frustbench",
            failed("error: device offline"),
        );
        assert!(fetch_mem_pss_bytes(&runner, "emulator-5554", "it.f0x.frustbench").is_err());
    }

    #[test]
    fn fetch_thermal_success() {
        let runner = FakeProcessRunner::new().with(
            format!("adb -s emulator-5554 shell {THERMAL_SHELL_SCRIPT}"),
            ok("/sys/class/thermal/thermal_zone0|cpu-0|41000\n/sys/class/thermal/thermal_zone1||\n"),
        );
        let readings = fetch_thermal(&runner, "emulator-5554").unwrap();
        assert_eq!(
            readings,
            vec![ThermalReading {
                zone_label: "cpu-0".to_string(),
                millideg_c: 41000
            }]
        );
    }

    #[test]
    fn fetch_thermal_failed_exit_errs() {
        let runner = FakeProcessRunner::new().with(
            format!("adb -s emulator-5554 shell {THERMAL_SHELL_SCRIPT}"),
            failed("error: closed"),
        );
        assert!(fetch_thermal(&runner, "emulator-5554").is_err());
    }

    #[test]
    fn fetch_net_success() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell cat /proc/net/dev",
            ok("Inter-|   Receive                                                |  Transmit\n \
                face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
                lo: 100 0 0 0 0 0 0 0 100 0 0 0 0 0 0 0\n\
                wlan0: 5000 0 0 0 0 0 0 0 3000 0 0 0 0 0 0 0\n"),
        );
        assert_eq!(fetch_net(&runner, "emulator-5554").unwrap(), (5000, 3000));
    }

    #[test]
    fn fetch_net_garbage_output_errs() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell cat /proc/net/dev",
            ok("not the right format"),
        );
        assert!(fetch_net(&runner, "emulator-5554").is_err());
    }

    #[test]
    fn fetch_net_failed_exit_errs() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 shell cat /proc/net/dev",
            failed("error: device offline"),
        );
        assert!(fetch_net(&runner, "emulator-5554").is_err());
    }

    #[test]
    fn fetch_all_is_best_effort_when_every_probe_fails() {
        // No responses registered at all — every probe fails to match and
        // errors; `fetch_all` must still return a (fully-empty) snapshot
        // rather than propagating any single failure.
        let runner = FakeProcessRunner::new();
        let snapshot = fetch_all(&runner, "emulator-5554", "4021", "it.f0x.frustbench");
        assert!(snapshot.cpu_ticks.is_none());
        assert!(snapshot.mem_rss_bytes.is_none());
        assert!(snapshot.thermal.is_empty());
        assert!(snapshot.net.is_none());
    }
}
