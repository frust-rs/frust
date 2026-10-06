//! S8 — Plugin-call overhead (`frust-shared-preferences` write+read loops).
//!
//! The head-to-head for the S8 claim under test: direct in-process FFI
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
//!   Flutter side, which must separate the two).
//! - **Error accounting.** `write_one`'s `Result` is no longer discarded: a
//!   failed write increments a running `write_errors` count and marks that
//!   key as write-failed so the read phase never double-counts it. Every
//!   read verifies its value against the deterministic value `write_one`
//!   should have written, counting an unexpectedly-absent key
//!   (`read_unexpected_none`) or a present-but-wrong value
//!   (`read_value_mismatch`) separately — skipping verification (not the
//!   timing) for a key whose write already failed. Each per-op `bench_emit`
//!   line carries an `err=0|1` field; a nonzero total across any of the
//!   three counters emits one parseable `s8-errors` marker line so the
//!   harness can flag the run.
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
    Align, Alignment, AnyView, AsyncValue, Component, Get, Stack, UseTask, View, any, component,
    text, use_task,
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

    /// A stable, dense index into a per-type slot (write-success tracking) —
    /// order matches [`PrefType::ALL`].
    fn index(self) -> usize {
        match self {
            PrefType::Bool => 0,
            PrefType::I64 => 1,
            PrefType::F64 => 2,
            PrefType::Str => 3,
            PrefType::StrList => 4,
        }
    }
}

/// The deterministic value `write_one` writes (and `verify_read` expects
/// back) for `(ty, i)` — factored out so the write and read-verify paths can
/// never drift from each other.
#[derive(Clone, PartialEq, Debug)]
enum ExpectedValue {
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
    StrList(Vec<String>),
}

fn expected_value(ty: PrefType, i: usize) -> ExpectedValue {
    match ty {
        PrefType::Bool => ExpectedValue::Bool(i.is_multiple_of(2)),
        PrefType::I64 => ExpectedValue::I64(i as i64),
        PrefType::F64 => ExpectedValue::F64(i as f64 * 1.5),
        PrefType::Str => ExpectedValue::Str(format!("value_{i}")),
        PrefType::StrList => {
            ExpectedValue::StrList(vec![format!("a{i}"), format!("b{i}"), format!("c{i}")])
        }
    }
}

/// The outcome of verifying one read against its [`expected_value`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReadCheck {
    /// The stored value matched exactly.
    Match,
    /// The key had no value at all, though its write was never marked
    /// failed — a real plugin-boundary problem (or a store that silently
    /// dropped the write).
    UnexpectedNone,
    /// The key had a value, but not the one `write_one` should have
    /// written for `(ty, i)`.
    ValueMismatch,
}

/// The outcome of one S8 run, surfaced as the status readout.
#[derive(Clone)]
pub struct S8Report {
    writes: usize,
    reads: usize,
    /// Writes whose `write_one` call returned `Err` (per-type + total counts
    /// are recoverable from the per-op `bench_emit` lines' `type=`/`err=`
    /// fields; this is the summed total).
    write_errors: usize,
    /// Reads that found no value at a key whose write wasn't already
    /// counted as failed.
    read_unexpected_none: usize,
    /// Reads that found a value not matching [`expected_value`] at a key
    /// whose write wasn't already counted as failed.
    read_value_mismatch: usize,
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

    fn build(&self, state: &mut S8State) -> impl View<S8State> {
        let status = match state.task.signal().get() {
            AsyncValue::Idle | AsyncValue::Loading(_) => {
                format!("running plugin write/read loops ({KEYS_PER_TYPE} keys × 5 types)…")
            }
            AsyncValue::Ready(report) => match report.error {
                Some(msg) => format!("S8 backend unavailable: {msg}"),
                None => {
                    let errors = report.write_errors
                        + report.read_unexpected_none
                        + report.read_value_mismatch;
                    format!(
                        "S8 complete — {} writes, {} reads ({}){}",
                        report.writes,
                        report.reads,
                        if state.burst {
                            "burst-during-animation"
                        } else {
                            "quiescent"
                        },
                        if errors > 0 {
                            format!(
                                " — {errors} error(s): write={} unexpected_none={} mismatch={}",
                                report.write_errors,
                                report.read_unexpected_none,
                                report.read_value_mismatch
                            )
                        } else {
                            String::new()
                        },
                    )
                }
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
                write_errors: 0,
                read_unexpected_none: 0,
                read_value_mismatch: 0,
                error: Some(e.to_string()),
            };
        }
    };

    // Per-key write-success tracking (flat, `ty.index() * KEYS_PER_TYPE + i`)
    // so the read phase can skip verification — never the timing — for a key
    // whose write already failed, per this scenario's no-double-counting
    // contract.
    let mut write_ok = vec![true; PrefType::ALL.len() * KEYS_PER_TYPE];

    // --- writes (every setX crosses the boundary — the headline number) ---
    frust_shell_common::perf::mark_scenario_start("s8-write");
    let mut write_total_us: u128 = 0;
    let mut writes = 0;
    let mut write_errors = 0;
    for ty in PrefType::ALL {
        for i in 0..KEYS_PER_TYPE {
            let key = format!("s8_{}_{i}", ty.tag());
            let t = Instant::now();
            let result = write_one(&prefs, ty, &key, i);
            let us = t.elapsed().as_micros();
            write_total_us += us;
            writes += 1;
            let err = result.is_err();
            if err {
                write_errors += 1;
                write_ok[ty.index() * KEYS_PER_TYPE + i] = false;
            }
            frust_shell_common::perf::bench_emit(&format!(
                "frust-perf plugin scenario=s8-write op=write type={} n={i} us={us} err={}",
                ty.tag(),
                err as u8
            ));
        }
    }
    frust_shell_common::perf::bench_emit(&format!(
        "frust-perf plugin op=write type=total n={writes} us={write_total_us} errors={write_errors}"
    ));
    frust_shell_common::perf::mark_scenario_end("s8-write");

    // --- reads (a real in-process backend call each — no cache layer),
    // each verified against the value write_one should have written ---
    frust_shell_common::perf::mark_scenario_start("s8-read");
    let mut read_total_us: u128 = 0;
    let mut reads = 0;
    let mut read_unexpected_none = 0;
    let mut read_value_mismatch = 0;
    for ty in PrefType::ALL {
        for i in 0..KEYS_PER_TYPE {
            let key = format!("s8_{}_{i}", ty.tag());
            let t = Instant::now();
            let got = read_one(&prefs, ty, &key);
            let us = t.elapsed().as_micros();
            // verification deliberately excluded from the timed window — do
            // not reintroduce
            let check = verify_read(ty, i, got);
            read_total_us += us;
            reads += 1;
            // Skip attributing a mismatch/none to this key if its write
            // already failed — that failure is already counted above, and
            // charging it again here would double-count the same root
            // cause.
            let write_already_failed = !write_ok[ty.index() * KEYS_PER_TYPE + i];
            let err = !write_already_failed && check != ReadCheck::Match;
            if err {
                match check {
                    ReadCheck::UnexpectedNone => read_unexpected_none += 1,
                    ReadCheck::ValueMismatch => read_value_mismatch += 1,
                    ReadCheck::Match => unreachable!("err is only true for a non-Match check"),
                }
            }
            frust_shell_common::perf::bench_emit(&format!(
                "frust-perf plugin scenario=s8-read op=read type={} n={i} us={us} err={}",
                ty.tag(),
                err as u8
            ));
        }
    }
    frust_shell_common::perf::bench_emit(&format!(
        "frust-perf plugin op=read type=total n={reads} us={read_total_us} errors={}",
        read_unexpected_none + read_value_mismatch
    ));
    frust_shell_common::perf::mark_scenario_end("s8-read");

    let total_errors = write_errors + read_unexpected_none + read_value_mismatch;
    if total_errors > 0 {
        frust_shell_common::perf::bench_emit(&format!(
            "frust-perf plugin s8-errors write_errors={write_errors} \
             read_unexpected_none={read_unexpected_none} read_value_mismatch={read_value_mismatch}"
        ));
    }

    S8Report {
        writes,
        reads,
        write_errors,
        read_unexpected_none,
        read_value_mismatch,
        error: None,
    }
}

/// Write one value of `ty` under `key`, deriving a deterministic value from `i`
/// via [`expected_value`] (matching the Flutter side's per-type value shapes).
fn write_one(
    prefs: &SharedPreferences,
    ty: PrefType,
    key: &str,
    i: usize,
) -> Result<(), frust_shared_preferences::PrefsError> {
    match expected_value(ty, i) {
        ExpectedValue::Bool(v) => prefs.set_bool(key, v),
        ExpectedValue::I64(v) => prefs.set_i64(key, v),
        ExpectedValue::F64(v) => prefs.set_f64(key, v),
        ExpectedValue::Str(v) => prefs.set_string(key, v),
        ExpectedValue::StrList(v) => prefs.set_string_list(key, v),
    }
}

/// The raw value read back for a key, one variant per [`PrefType`] — captured
/// inside the timed window by [`read_one`]; compared against
/// [`expected_value`] only in [`verify_read`], called after the timed window
/// has already closed.
enum ReadValue {
    Bool(Option<bool>),
    I64(Option<i64>),
    F64(Option<f64>),
    Str(Option<String>),
    StrList(Option<Vec<String>>),
}

/// Read one value of `ty` under `key` — the raw backend call only, `black_box`ed
/// so the optimizer can't elide the boundary call.
///
/// Verification deliberately excluded from the timed window — do not
/// reintroduce. Comparing the result against `expected_value` is CPU work
/// unrelated to the backend boundary being measured; see [`verify_read`],
/// called only once `Instant::elapsed` has already captured `us` in
/// `run_prefs_bench`.
fn read_one(prefs: &SharedPreferences, ty: PrefType, key: &str) -> ReadValue {
    match ty {
        PrefType::Bool => ReadValue::Bool(black_box(prefs.get_bool(key))),
        PrefType::I64 => ReadValue::I64(black_box(prefs.get_i64(key))),
        PrefType::F64 => ReadValue::F64(black_box(prefs.get_f64(key))),
        PrefType::Str => ReadValue::Str(black_box(prefs.get_string(key))),
        PrefType::StrList => ReadValue::StrList(black_box(prefs.get_string_list(key))),
    }
}

/// Verify a [`read_one`] result against [`expected_value`]`(ty, i)` — called
/// after the timed window has already closed, so verification cost never
/// inflates the reported read latency (see `read_one`'s guard comment).
fn verify_read(ty: PrefType, i: usize, got: ReadValue) -> ReadCheck {
    let expected = expected_value(ty, i);
    match (got, expected) {
        (ReadValue::Bool(None), _)
        | (ReadValue::I64(None), _)
        | (ReadValue::F64(None), _)
        | (ReadValue::Str(None), _)
        | (ReadValue::StrList(None), _) => ReadCheck::UnexpectedNone,
        (ReadValue::Bool(Some(v)), ExpectedValue::Bool(e)) if v == e => ReadCheck::Match,
        (ReadValue::I64(Some(v)), ExpectedValue::I64(e)) if v == e => ReadCheck::Match,
        (ReadValue::F64(Some(v)), ExpectedValue::F64(e)) if v == e => ReadCheck::Match,
        (ReadValue::Str(Some(v)), ExpectedValue::Str(e)) if v == e => ReadCheck::Match,
        (ReadValue::StrList(Some(v)), ExpectedValue::StrList(e)) if v == e => ReadCheck::Match,
        _ => ReadCheck::ValueMismatch,
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

    #[test]
    fn pref_type_index_is_dense_and_unique() {
        // `write_ok`'s flat indexing (`ty.index() * KEYS_PER_TYPE + i`)
        // depends on `index()` being a bijection onto `0..PrefType::ALL.len()`.
        let mut seen: Vec<usize> = PrefType::ALL.iter().map(|t| t.index()).collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);
    }

    // The error/mismatch counters can't be exercised via an injected failing
    // store — `SharedPreferences::standard()` is the crate's sole public
    // constructor and its `Backend` trait is crate-private (see
    // `plugins/shared-preferences/src/lib.rs`), so there is no seam from this
    // out-of-tree bench crate to force a write failure. The counters are
    // instead exercised via the derived-value mismatch path:
    // `read_and_verify` (the test-only `read_one` + `verify_read`
    // composition below) is called against a deliberately wrong `i` (or an
    // absent key) on the real desktop backend, so the *verification logic*
    // — the error-accounting mechanism itself — is directly covered even
    // though a genuine backend I/O failure isn't reproducible here.

    /// Test-only composition mirroring `run_prefs_bench`'s two-step
    /// read-then-verify shape (`read_one` timed, `verify_read` after) without
    /// needing a real `Instant` in these assertions.
    fn read_and_verify(prefs: &SharedPreferences, ty: PrefType, key: &str, i: usize) -> ReadCheck {
        verify_read(ty, i, read_one(prefs, ty, key))
    }

    #[test]
    fn read_and_verify_matches_a_faithful_roundtrip() {
        let Ok(prefs) = SharedPreferences::standard() else {
            return;
        };
        let key = "s8_test_verify_match";
        write_one(&prefs, PrefType::Str, key, 42).unwrap();
        assert_eq!(
            read_and_verify(&prefs, PrefType::Str, key, 42),
            ReadCheck::Match
        );
        let _ = prefs.remove(key);
    }

    #[test]
    fn read_and_verify_detects_value_mismatch() {
        let Ok(prefs) = SharedPreferences::standard() else {
            return;
        };
        let key = "s8_test_verify_mismatch";
        // Write the value expected for i=7, but verify against i=8's expected
        // value — a stand-in for a backend that silently stored/returned the
        // wrong value, without needing an injectable failing store.
        write_one(&prefs, PrefType::I64, key, 7).unwrap();
        assert_eq!(
            read_and_verify(&prefs, PrefType::I64, key, 8),
            ReadCheck::ValueMismatch
        );
        let _ = prefs.remove(key);
    }

    #[test]
    fn read_and_verify_detects_unexpected_none() {
        let Ok(prefs) = SharedPreferences::standard() else {
            return;
        };
        let key = "s8_test_verify_unexpected_none";
        // Never written (or already removed) — a stand-in for a backend that
        // silently dropped the write.
        let _ = prefs.remove(key);
        assert_eq!(
            read_and_verify(&prefs, PrefType::Bool, key, 3),
            ReadCheck::UnexpectedNone
        );
    }

    #[test]
    fn read_and_verify_covers_every_pref_type() {
        // One faithful roundtrip per type, so the per-variant match arms in
        // `verify_read` (bool/i64/f64/string/string_list) are each exercised,
        // not just the i64/string cases above.
        let Ok(prefs) = SharedPreferences::standard() else {
            return;
        };
        for ty in PrefType::ALL {
            let key = format!("s8_test_verify_all_{}", ty.tag());
            write_one(&prefs, ty, &key, 5).unwrap();
            assert_eq!(
                read_and_verify(&prefs, ty, &key, 5),
                ReadCheck::Match,
                "type {} failed to roundtrip",
                ty.tag()
            );
            let _ = prefs.remove(&key);
        }
    }
}
