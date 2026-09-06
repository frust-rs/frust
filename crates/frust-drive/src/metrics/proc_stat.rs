//! Pure parsing for Linux's `/proc/<pid>/stat` CPU-jiffy fields — shared by
//! [`super::desktop`] (reads the file directly) and [`super::android`] (reads
//! it via `adb shell cat`), since the file's shape is identical either way.

use std::time::Duration;

use super::MetricsError;

/// `utime`+`stime` read from `/proc/<pid>/stat` fields 14/15, in kernel
/// jiffies — meaningless as an absolute value; only a delta between two
/// readings (see [`cpu_percent_from_ticks`]) is useful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTicks {
    pub utime: u64,
    pub stime: u64,
}

/// The near-universal Linux `CLK_TCK` (jiffies-per-second) value —
/// `sysconf(_SC_CLK_TCK)` returns `100` on every mainstream Linux
/// target (x86, ARM, RISC-V) this crate ships to; `proc(5)` documents the
/// scaling factor without guaranteeing a fixed value, but this crate has no
/// portable `std`-only way to query `sysconf` (querying it for real needs
/// `libc`, and `frust-drive` carries zero new dependencies for this feature —
/// see the module doc). Revisit if a real target with a non-100 `CLK_TCK`
/// (historically only some embedded/Alpha kernels) is ever supported.
pub const LINUX_CLK_TCK: u64 = 100;

/// Parses `/proc/<pid>/stat`'s `utime`/`stime` fields (14/15) out of its raw
/// content.
///
/// `stat`'s second field (`comm`, the process name in parens) can itself
/// contain spaces AND unmatched `(`/`)` characters (a thread renamed via
/// `prctl(PR_SET_NAME, ...)` can pick almost any byte string) — the only safe
/// way to locate the fixed numeric fields that follow is to split after the
/// **last** `)` in the whole line, never the first. Everything after that
/// point is guaranteed plain whitespace-separated fields with no embedded
/// parens.
pub fn parse_proc_stat_ticks(contents: &str) -> Result<CpuTicks, MetricsError> {
    let line = contents.trim();
    let last_paren = line
        .rfind(')')
        .ok_or_else(|| MetricsError::parse("proc/pid/stat", "no `)` found (missing comm field)"))?;
    let rest = &line[last_paren + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();

    // Post-`)` field indices (0-based): 0=state(3) 1=ppid(4) 2=pgrp(5)
    // 3=session(6) 4=tty_nr(7) 5=tpgid(8) 6=flags(9) 7=minflt(10)
    // 8=cminflt(11) 9=majflt(12) 10=cmajflt(13) 11=utime(14) 12=stime(15).
    let utime = field_u64(&fields, 11, "utime (field 14)")?;
    let stime = field_u64(&fields, 12, "stime (field 15)")?;
    Ok(CpuTicks { utime, stime })
}

fn field_u64(fields: &[&str], index: usize, name: &'static str) -> Result<u64, MetricsError> {
    let raw = fields
        .get(index)
        .ok_or_else(|| MetricsError::parse("proc/pid/stat", format!("missing {name}")))?;
    raw.parse::<u64>()
        .map_err(|e| MetricsError::parse("proc/pid/stat", format!("{name} `{raw}`: {e}")))
}

/// CPU utilization since the previous reading, as a percentage of one core
/// (`100.0` == one core fully busy for the whole `elapsed` window). Pure:
/// takes both readings and the wall-clock gap between them rather than
/// touching a clock itself, so it's testable without real timing.
pub fn cpu_percent_from_ticks(prev: CpuTicks, curr: CpuTicks, elapsed: Duration) -> f32 {
    if elapsed.is_zero() {
        return 0.0;
    }
    let prev_total = prev.utime.saturating_add(prev.stime);
    let curr_total = curr.utime.saturating_add(curr.stime);
    let delta_ticks = curr_total.saturating_sub(prev_total);
    let delta_secs = delta_ticks as f64 / LINUX_CLK_TCK as f64;
    let percent = (delta_secs / elapsed.as_secs_f64()) * 100.0;
    percent.max(0.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-shaped `/proc/<pid>/stat` line, well-formed comm.
    const NORMAL_STAT: &str = "4021 (frustbench) S 1 4021 4021 0 -1 4194560 227 0 0 0 42 17 0 0 20 0 4 0 12345678 123456789 1234 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0 0 0 0 0 0 0 0 0";

    /// The pathological trap: `comm` itself contains `(`/`)`/spaces (a
    /// `prctl(PR_SET_NAME, ...)`-renamed thread), so a naive "split on the
    /// FIRST `)`" parse would misparse every field after it.
    const PATHOLOGICAL_COMM_STAT: &str = "4022 (my (proc) name) S 1 4021 4021 0 -1 4194560 227 0 0 0 42 17 0 0 20 0 1 0 12345678 12345678 1234 18446744073709551615 1 1 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0 0 0 0 0 0 0 0 0";

    #[test]
    fn parses_utime_stime_from_a_normal_stat_line() {
        let ticks = parse_proc_stat_ticks(NORMAL_STAT).unwrap();
        assert_eq!(
            ticks,
            CpuTicks {
                utime: 42,
                stime: 17
            }
        );
    }

    #[test]
    fn parses_utime_stime_past_a_pathological_comm_field() {
        let ticks = parse_proc_stat_ticks(PATHOLOGICAL_COMM_STAT).unwrap();
        assert_eq!(
            ticks,
            CpuTicks {
                utime: 42,
                stime: 17
            }
        );
    }

    #[test]
    fn missing_closing_paren_errs() {
        assert!(parse_proc_stat_ticks("4021 frustbench S 1 4021").is_err());
    }

    #[test]
    fn truncated_fields_after_comm_errs() {
        assert!(parse_proc_stat_ticks("4021 (frustbench) S 1 4021").is_err());
    }

    #[test]
    fn cpu_percent_zero_delta_is_zero() {
        let ticks = CpuTicks {
            utime: 100,
            stime: 50,
        };
        assert_eq!(
            cpu_percent_from_ticks(ticks, ticks, Duration::from_secs(1)),
            0.0
        );
    }

    #[test]
    fn cpu_percent_full_core_for_a_one_second_window() {
        // 100 ticks (LINUX_CLK_TCK) of combined utime+stime over exactly one
        // elapsed second == 100% of one core.
        let prev = CpuTicks { utime: 0, stime: 0 };
        let curr = CpuTicks {
            utime: 60,
            stime: 40,
        };
        let percent = cpu_percent_from_ticks(prev, curr, Duration::from_secs(1));
        assert!((percent - 100.0).abs() < 0.01, "got {percent}");
    }

    #[test]
    fn cpu_percent_zero_elapsed_never_divides_by_zero() {
        let prev = CpuTicks { utime: 0, stime: 0 };
        let curr = CpuTicks {
            utime: 10,
            stime: 10,
        };
        assert_eq!(cpu_percent_from_ticks(prev, curr, Duration::ZERO), 0.0);
    }
}
