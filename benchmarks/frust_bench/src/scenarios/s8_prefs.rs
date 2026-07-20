//! S8 — Plugin-call overhead (`frust-shared-preferences` write+read loops).
//!
//! The head-to-head for PLAN 9.E's S8 claim: direct in-process FFI
//! (`frust-shared-preferences`'s `objc2`/`jni`/file backend, zero codec) vs
//! Flutter's `shared_preferences` MethodChannel round-trip. This scenario runs
//! write + read loops across all five value types (`bool`/`i64`/`f64`/`String`/
//! `Vec<String>`), timing each op and logging it as a parseable line, plus a
//! total wall time per phase — mirroring the Flutter bench's `s8_prefs.dart`.
//!
//! # S8 fairness (PROTOCOL §8)
//!
//! - **Unique keys.** Each `(type, i)` is a distinct key (`s8_<type>_<i>`), so a
//!   read can never be served from a stale in-memory entry — matching the
//!   Flutter rule that keeps a read from being a Dart-map lookup.
//! - **Writes and reads reported separately.** The `s8-write` and `s8-read`
//!   marker windows (and `op=write` / `op=read` per-op lines) are distinct
//!   series. Every write crosses the boundary on both frameworks, so write
//!   latency is the headline number.
//! - **No cache layer on the frust side.** `frust-shared-preferences` has no
//!   Dart-style read cache, so every read is a real in-process backend call —
//!   there is no `read_cached` vs `read_crossing` split to make (unlike the
//!   Flutter side, which must separate the two). This divergence is noted in the
//!   task's completion summary.
//!
//! The whole loop runs on a blocking-pool thread (`spawn_blocking`), so the
//! optional burst-during-animation variant's UI-thread animation keeps running
//! throughout — enable it with `FRUST_BENCH_S8_BURST=1` (the frust analog of the
//! Flutter side's `--dart-define=S8_BURST=1`). Desktop uses the plugin's macOS
//! `NSUserDefaults` backend (or the file backend on Linux/Windows) — fine for
//! the smoke; the measured runs are on-device.

use std::hint::black_box;
use std::time::Instant;

use frust::{
    Align, Alignment, AnyView, AsyncValue, Component, Get, Stack, UseTask, any, component, text,
    use_task,
};
use frust_shared_preferences::SharedPreferences;

use super::s4_heavy::SpinBox;
use super::{BenchState, Scenario};

/// Env var enabling the burst-during-animation variant (frust analog of the
/// Flutter side's `--dart-define=S8_BURST=1`).
const S8_BURST_ENV: &str = "FRUST_BENCH_S8_BURST";

/// Unique keys per value type — enough per-op samples for stable percentiles,
/// matching the Flutter side's `_keysPerType = 200`.
const KEYS_PER_TYPE: usize = 200;

/// S8 — Plugin-call overhead.
pub struct S8;

impl Scenario for S8 {
    fn id(&self) -> &'static str {
        "s8"
    }

    fn title(&self) -> &'static str {
        "Plugin-call overhead (shared_preferences)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        any(component(S8Prefs))
    }
}

/// The five preference value types, each with a key-name tag matching the
/// Flutter side's `_PrefType.name`.
#[derive(Clone, Copy)]
enum PrefType {
    Bool,
    I64,
    F64,
    Str,
    StrList,
}

impl PrefType {
    const ALL: [PrefType; 5] = [
        PrefType::Bool,
        PrefType::I64,
        PrefType::F64,
        PrefType::Str,
        PrefType::StrList,
    ];

    fn tag(self) -> &'static str {
        match self {
            PrefType::Bool => "bool",
            PrefType::I64 => "i64",
            PrefType::F64 => "f64",
            PrefType::Str => "string",
            PrefType::StrList => "string_list",
        }
    }
}

/// The outcome of one S8 run, surfaced as the status readout.
#[derive(Clone)]
pub struct S8Report {
    writes: usize,
    reads: usize,
    /// A note when the backend was unavailable (e.g. an old scaffold with no
    /// `nativeInitPlatform` on Android) — S8 then reports zero ops rather than
    /// crashing, per the graceful-error contract.
    error: Option<String>,
}

/// S8 — Plugin-call overhead component.
pub struct S8Prefs;

/// Retained S8 state: the run task plus whether the burst variant is active.
pub struct S8State {
    task: UseTask<S8Report>,
    burst: bool,
}

impl Component for S8Prefs {
    type State = S8State;

    fn init(&self) -> S8State {
        let burst = std::env::var(S8_BURST_ENV)
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false);
        // The whole loop runs on a blocking-pool thread, so a burst-variant
        // UI-thread animation keeps running throughout. The error type is
        // inferred (tokio's `JoinError`), so this crate never names `tokio`.
        let task = use_task(|| async { frust::spawn_blocking(run_prefs_bench).await });
        S8State { task, burst }
    }

    fn build(&self, state: &mut S8State) -> AnyView<S8State> {
        let status = match state.task.signal().get() {
            AsyncValue::Idle | AsyncValue::Loading(_) => {
                format!("running plugin write/read loops ({KEYS_PER_TYPE} keys × 5 types)…")
            }
            AsyncValue::Ready(report) => match report.error {
                Some(msg) => format!("S8 backend unavailable: {msg}"),
                None => format!(
                    "S8 complete — {} writes, {} reads ({})",
                    report.writes,
                    report.reads,
                    if state.burst {
                        "burst-during-animation"
                    } else {
                        "quiescent"
                    },
                ),
            },
            AsyncValue::Error(_) => "S8 run task failed".to_string(),
        };

        let mut layers: Vec<AnyView<S8State>> = Vec::new();
        // Burst variant: an S1-style animation runs on the UI thread while the
        // plugin loop hammers the boundary off-thread.
        if state.burst {
            layers.push(any(SpinBox));
        }
        layers.push(any(Align(
            Alignment::new(0.0, 0.0),
            text(status).size(18.0),
        )));
        any(Stack(layers))
    }
}

/// The measured loop (runs off the UI thread). Opens the store, times every
/// write and read, logs each as a parseable line, and brackets each phase with
/// a marker pair for the phase's total wall time.
fn run_prefs_bench() -> S8Report {
    let prefs = match SharedPreferences::standard() {
        Ok(p) => p,
        Err(e) => {
            return S8Report {
                writes: 0,
                reads: 0,
                error: Some(e.to_string()),
            };
        }
    };

    // --- writes (every setX crosses the boundary — the headline number) ---
    frust_shell_common::perf::mark_scenario_start("s8-write");
    let mut write_total_us: u128 = 0;
    let mut writes = 0;
    for ty in PrefType::ALL {
        for i in 0..KEYS_PER_TYPE {
            let key = format!("s8_{}_{i}", ty.tag());
            let t = Instant::now();
            let _ = write_one(&prefs, ty, &key, i);
            let us = t.elapsed().as_micros();
            write_total_us += us;
            writes += 1;
            frust_shell_common::perf::bench_emit(&format!(
                "frust-perf plugin op=write type={} n={i} us={us}",
                ty.tag()
            ));
        }
    }
    frust_shell_common::perf::bench_emit(&format!(
        "frust-perf plugin op=write type=total n={writes} us={write_total_us}"
    ));
    frust_shell_common::perf::mark_scenario_end("s8-write");

    // --- reads (a real in-process backend call each — no cache layer) ---
    frust_shell_common::perf::mark_scenario_start("s8-read");
    let mut read_total_us: u128 = 0;
    let mut reads = 0;
    for ty in PrefType::ALL {
        for i in 0..KEYS_PER_TYPE {
            let key = format!("s8_{}_{i}", ty.tag());
            let t = Instant::now();
            read_one(&prefs, ty, &key);
            let us = t.elapsed().as_micros();
            read_total_us += us;
            reads += 1;
            frust_shell_common::perf::bench_emit(&format!(
                "frust-perf plugin op=read type={} n={i} us={us}",
                ty.tag()
            ));
        }
    }
    frust_shell_common::perf::bench_emit(&format!(
        "frust-perf plugin op=read type=total n={reads} us={read_total_us}"
    ));
    frust_shell_common::perf::mark_scenario_end("s8-read");

    S8Report {
        writes,
        reads,
        error: None,
    }
}

/// Write one value of `ty` under `key`, deriving a deterministic value from `i`
/// (matching the Flutter side's per-type value shapes).
fn write_one(
    prefs: &SharedPreferences,
    ty: PrefType,
    key: &str,
    i: usize,
) -> Result<(), frust_shared_preferences::PrefsError> {
    match ty {
        PrefType::Bool => prefs.set_bool(key, i.is_multiple_of(2)),
        PrefType::I64 => prefs.set_i64(key, i as i64),
        PrefType::F64 => prefs.set_f64(key, i as f64 * 1.5),
        PrefType::Str => prefs.set_string(key, format!("value_{i}")),
        PrefType::StrList => {
            prefs.set_string_list(key, vec![format!("a{i}"), format!("b{i}"), format!("c{i}")])
        }
    }
}

/// Read one value of `ty` under `key`; the result is `black_box`ed so the
/// optimizer can't elide the boundary call.
fn read_one(prefs: &SharedPreferences, ty: PrefType, key: &str) {
    match ty {
        PrefType::Bool => {
            black_box(prefs.get_bool(key));
        }
        PrefType::I64 => {
            black_box(prefs.get_i64(key));
        }
        PrefType::F64 => {
            black_box(prefs.get_f64(key));
        }
        PrefType::Str => {
            black_box(prefs.get_string(key));
        }
        PrefType::StrList => {
            black_box(prefs.get_string_list(key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pref_type_tags_match_flutter_names() {
        let tags: Vec<&str> = PrefType::ALL.iter().map(|t| t.tag()).collect();
        assert_eq!(tags, ["bool", "i64", "f64", "string", "string_list"]);
    }

    #[test]
    fn keys_are_unique_per_type_and_index() {
        // No two (type, i) pairs collide — the read-can't-be-cached guarantee.
        let mut keys = std::collections::HashSet::new();
        for ty in PrefType::ALL {
            for i in 0..KEYS_PER_TYPE {
                assert!(
                    keys.insert(format!("s8_{}_{i}", ty.tag())),
                    "duplicate key for {}/{i}",
                    ty.tag()
                );
            }
        }
        assert_eq!(keys.len(), KEYS_PER_TYPE * 5);
    }

    #[test]
    fn write_and_read_roundtrip_on_the_desktop_backend() {
        // Desktop uses the NSUserDefaults (macOS) or file (Linux/Windows)
        // backend; a single roundtrip must survive.
        let Ok(prefs) = SharedPreferences::standard() else {
            // No backend on this host (e.g. an uninitialized platform) — the
            // graceful-error path is covered by `run_prefs_bench`'s Err arm.
            return;
        };
        write_one(&prefs, PrefType::I64, "s8_test_roundtrip", 7).unwrap();
        assert_eq!(prefs.get_i64("s8_test_roundtrip"), Some(7));
        let _ = prefs.remove("s8_test_roundtrip");
    }
}
