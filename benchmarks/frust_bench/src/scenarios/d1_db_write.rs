//! D1 — DB write latency (`frust-database`'s batched-transaction vs
//! single-row-autocommit insert paths — `benchmarks/PROTOCOL.md` §9.3).
//!
//! Unlike the `s1..=s8` frame-class scenarios, D1 has no per-frame render
//! series of its own — its headline metric is per-op latency (§7's per-op
//! contract), the same `d*`-namespace shape §9.1 declares. Each run: opens a
//! fresh scratch database file (never the app's real `frust-database`
//! standard location — see [`open_scratch_db`]), then performs, in order,
//! `BATCH_REPS` batched-transaction inserts of `BATCH_N` rows each (table
//! dropped and recreated fresh before every rep, so batch cost is never
//! inflated by a growing table across reps within a run), then `SINGLE_M`
//! single-row autocommit inserts into a once-more-freshly-recreated table.
//!
//! # Row shape / seed dataset (PROTOCOL §9.2, shared with D2)
//!
//! This module is the **canonical** home of the §9.2 generator: the Flutter
//! side's `flutter_bench/lib/scenarios/d1_db_write.dart` is a bit-for-bit
//! port of the three functions below, and both suites assert the identical
//! literal golden vectors (see this module's `tests`), so a parity break
//! fails on both sides rather than silently skewing a published number.
//!
//! [`row_name`]/[`row_value`]/[`row_payload`] are pure functions of
//! `(seed, i)` — no wall-clock or process-local RNG state — built from a
//! small `splitmix64` generator ([`splitmix64`]) seeded with [`ROW_SEED`].
//! D2 imports these three (plus [`splitmix64`], for its own deterministic
//! key permutation) rather than duplicating them, per this module's role as
//! the dataset owner.
//!
//! # Timed window (PROTOCOL §9.3)
//!
//! Row generation never happens inside a measured window: every op's
//! parameters are built *before* the [`Instant`] that times it (the whole
//! batch's parameters before the transaction starts), so a `us` value
//! covers the database call alone. Both apps obey this rule, which is what
//! keeps the two columns' `us` values comparable even though their
//! generators cost different amounts.
//!
//! # Engine (PROTOCOL §9.7)
//!
//! Never names an [`Engine`](frust_database::Engine) variant directly —
//! [`open_scratch_db`] calls [`Database::open_at`], which resolves this
//! crate's compiled default engine itself (`engine-sqlite` today; see
//! `plugins/database/src/lib.rs`'s module doc, *Engines*). A future
//! `engine-turso` column reaches this scenario with no scenario-code change,
//! only a manifest/feature change.
//!
//! # UI-thread discipline
//!
//! The whole bench body (`run_d1_bench`) runs on a blocking-pool thread via
//! `frust::spawn_blocking`, mirroring S8's `s8_prefs.rs` pairing — every
//! `frust-database` call is a blocking synchronous call (see
//! `plugins/database/src/lib.rs`'s module doc, *UI-thread discipline*), and
//! this scenario's minimal status view still renders on the UI thread while
//! the bench runs off it.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use frust::{
    Align, Alignment, AnyView, AsyncValue, Component, Get, UseTask, View, any, component, text,
    use_task,
};
use frust_database::{Database, DatabaseError, Value};

use super::{BenchState, Scenario};

/// Rows per `insert_batch` transaction (PROTOCOL §9.3, declared convention).
const BATCH_N: usize = 2_000;
/// `insert_batch` reps per run — each against a freshly dropped/recreated
/// table (PROTOCOL §9.3).
const BATCH_REPS: usize = 10;
/// Single-row autocommit `insert_single` ops per run, into a table dropped
/// and recreated once at the start of that phase (PROTOCOL §9.3).
const SINGLE_M: usize = 500;

/// Deterministic seed for the row generator (PROTOCOL §9.2) — an arbitrary
/// declared convention, not derived from any external standard.
pub(crate) const ROW_SEED: u64 = 424_242;

/// The `rows` table's DDL, shared by every phase of D1 and D2 — PROTOCOL
/// §9.2's fixed row shape (`id INTEGER PK`, `name TEXT ~64B`, `value REAL`,
/// `payload BLOB ~256B`).
const CREATE_ROWS_SQL: &str = "CREATE TABLE rows (\
    id INTEGER PRIMARY KEY, \
    name TEXT NOT NULL, \
    value REAL NOT NULL, \
    payload BLOB NOT NULL\
)";

/// One-row insert, positional params `(id, name, value, payload)`.
pub(crate) const INSERT_ROW_SQL: &str =
    "INSERT INTO rows (id, name, value, payload) VALUES (?1, ?2, ?3, ?4)";

/// D1 — DB write latency.
pub struct D1;

impl Scenario for D1 {
    fn id(&self) -> &'static str {
        "d1"
    }

    fn title(&self) -> &'static str {
        "DB writes (frust-database)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        any(component(D1Write))
    }
}

/// SplitMix64 — a small, fast, well-distributed non-cryptographic PRNG used
/// only to derive deterministic per-row bytes from `(seed, i)`. Shared by
/// [`row_name`]/[`row_value`]/[`row_payload`] and, via D2's import, its own
/// deterministic key-permutation.
pub(crate) fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Row `i`'s deterministic `name` — exactly 64 bytes, a pure function of
/// `(seed, i)`.
pub(crate) fn row_name(seed: u64, i: u64) -> String {
    let mut state = seed ^ i.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0x1111;
    let mut s = format!("row-{i:010}-");
    while s.len() < 64 {
        s.push_str(&format!("{:016x}", splitmix64(&mut state)));
    }
    s.truncate(64);
    s
}

/// Row `i`'s deterministic `value` (REAL) — a pure function of `(seed, i)`.
pub(crate) fn row_value(seed: u64, i: u64) -> f64 {
    let mut state = seed ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x2222;
    (splitmix64(&mut state) % 1_000_000) as f64 / 1000.0
}

/// Row `i`'s deterministic `payload` (BLOB) — exactly 256 pseudo-random
/// bytes, a pure function of `(seed, i)`.
pub(crate) fn row_payload(seed: u64, i: u64) -> Vec<u8> {
    let mut state = seed ^ i.wrapping_mul(0xD6E8_FEB8_6659_FD93) ^ 0x3333;
    let mut out = Vec::with_capacity(264);
    while out.len() < 256 {
        out.extend_from_slice(&splitmix64(&mut state).to_le_bytes());
    }
    out.truncate(256);
    out
}

/// Row `i`'s full parameter list for [`INSERT_ROW_SQL`], built from the
/// §9.2 generator seeded with [`ROW_SEED`].
pub(crate) fn row_params(i: u64) -> [Value; 4] {
    [
        Value::Integer(i as i64),
        Value::Text(row_name(ROW_SEED, i)),
        Value::Real(row_value(ROW_SEED, i)),
        Value::Blob(row_payload(ROW_SEED, i)),
    ]
}

/// Drop (if present) and recreate the `rows` table — PROTOCOL §9.3/§9.4's
/// "fresh empty table" step, factored out since both D1's two phases and
/// D2's pre-seed all need it.
pub(crate) fn recreate_rows_table(db: &Database) -> Result<(), DatabaseError> {
    db.execute("DROP TABLE IF EXISTS rows", ())?;
    db.execute(CREATE_ROWS_SQL, ())?;
    Ok(())
}

/// Open a fresh scratch database file under the OS temp dir for one d-class
/// run, tagged by `scenario` — mirrors `plugins/database/src/conformance.rs`'s
/// own `scratch_dir` precedent (never [`frust_paths::data_dir`]/this crate's
/// real standard location). Returns the open handle plus the file path, so
/// the caller can remove it (and its WAL/SHM sidecars) once the run
/// finishes — see [`ScratchCleanup`].
pub(crate) fn open_scratch_db(scenario: &str) -> Result<(Database, PathBuf), DatabaseError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "frustbench-{scenario}-{}-{n}.db",
        std::process::id()
    ));
    let db = Database::open_at(&path)?;
    Ok((db, path))
}

/// Best-effort removal of a [`open_scratch_db`] scratch file plus its WAL
/// journal-mode sidecars (`<path>-wal`/`<path>-shm` — every real backend
/// opens in WAL mode, see `plugins/database/src/lib.rs`'s module doc,
/// *Interop discipline*) on drop, so a run's temp file never survives it —
/// including across an early return or an unwind.
pub(crate) struct ScratchCleanup(pub PathBuf);

impl Drop for ScratchCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(self.0.with_extension("db-wal"));
        let _ = fs::remove_file(self.0.with_extension("db-shm"));
    }
}

/// The outcome of one D1 run, surfaced as the status readout.
#[derive(Clone)]
pub struct D1Report {
    batch_reps: usize,
    batch_errors: usize,
    single_inserts: usize,
    single_errors: usize,
    /// Set only if opening the scratch database itself failed — D1 then
    /// reports zero ops rather than panicking, mirroring S8's graceful-error
    /// contract.
    error: Option<String>,
}

/// D1 — DB write latency component.
pub struct D1Write;

/// Retained D1 state: just the run task (no burst/animation variant, unlike
/// S8 — PROTOCOL §9 declares no such variant for the `d*` class).
pub struct D1State {
    task: UseTask<D1Report>,
}

impl Component for D1Write {
    type State = D1State;

    fn init(&self) -> D1State {
        let task = use_task(|| async { frust::spawn_blocking(run_d1_bench).await });
        D1State { task }
    }

    fn build(&self, state: &mut D1State) -> impl View<D1State> {
        let status = match state.task.signal().get() {
            AsyncValue::Idle | AsyncValue::Loading(_) => format!(
                "running D1 write bench ({BATCH_REPS} batch reps × {BATCH_N} rows, \
                 {SINGLE_M} single inserts)…"
            ),
            AsyncValue::Ready(report) => match report.error {
                Some(msg) => format!("D1 backend unavailable: {msg}"),
                None => {
                    let errors = report.batch_errors + report.single_errors;
                    format!(
                        "D1 complete — {} batch reps ({BATCH_N} rows each), {} single inserts{}",
                        report.batch_reps,
                        report.single_inserts,
                        if errors > 0 {
                            format!(
                                " — {errors} error(s): batch={} single={}",
                                report.batch_errors, report.single_errors
                            )
                        } else {
                            String::new()
                        },
                    )
                }
            },
            AsyncValue::Error(_) => "D1 run task failed".to_string(),
        };

        any(Align(Alignment::new(0.0, 0.0), text(status).size(18.0)))
    }
}

/// The measured run (runs off the UI thread). Opens a fresh scratch
/// database, runs both phases in PROTOCOL §9.3's declared order, logs every
/// op as a canonical per-op line (§7), and brackets each phase with a
/// marker pair for the phase's total wall time.
fn run_d1_bench() -> D1Report {
    let (db, path) = match open_scratch_db("d1") {
        Ok(v) => v,
        Err(e) => {
            return D1Report {
                batch_reps: 0,
                batch_errors: 0,
                single_inserts: 0,
                single_errors: 0,
                error: Some(e.to_string()),
            };
        }
    };
    let _cleanup = ScratchCleanup(path);

    // --- BATCH_REPS × insert_batch, table dropped/recreated fresh each rep ---
    frust_shell_common::perf::mark_scenario_start("d1-insert-batch");
    let mut batch_reps = 0;
    let mut batch_errors = 0;
    for rep in 0..BATCH_REPS {
        if let Err(e) = recreate_rows_table(&db) {
            batch_reps += 1;
            batch_errors += 1;
            log::warn!("d1: recreate_rows_table failed before batch rep {rep}: {e}");
            frust_shell_common::perf::bench_emit(&format!(
                "frust-perf op scenario=d1 op=insert_batch n={rep} us=0 err=1 rows={BATCH_N}"
            ));
            continue;
        }
        // Generated before the timer starts (PROTOCOL §9.3): the measured
        // window is the transaction alone, never the §9.2 generator.
        let batch_params: Vec<[Value; 4]> = (0..BATCH_N).map(|i| row_params(i as u64)).collect();
        let t = Instant::now();
        let result = db.transaction(move |txn| {
            for params in batch_params {
                txn.execute(INSERT_ROW_SQL, params)?;
            }
            Ok(())
        });
        let us = t.elapsed().as_micros();
        batch_reps += 1;
        let err = result.is_err();
        if err {
            batch_errors += 1;
        }
        frust_shell_common::perf::bench_emit(&format!(
            "frust-perf op scenario=d1 op=insert_batch n={rep} us={us} err={} rows={BATCH_N}",
            err as u8
        ));
    }
    frust_shell_common::perf::mark_scenario_end("d1-insert-batch");

    // --- drop/recreate once, then SINGLE_M × insert_single (autocommit) ---
    let mut single_inserts = 0;
    let mut single_errors = 0;
    if let Err(e) = recreate_rows_table(&db) {
        log::warn!("d1: recreate_rows_table failed before single-insert phase: {e}");
        frust_shell_common::perf::bench_emit(&format!(
            "frust-perf plugin d1-errors insert_batch_errors={batch_errors} \
             insert_single_errors={SINGLE_M}"
        ));
        return D1Report {
            batch_reps,
            batch_errors,
            single_inserts: 0,
            single_errors: SINGLE_M,
            error: None,
        };
    }
    // Same rule as the batch phase: the whole phase's parameters are
    // generated up front, outside every timer (PROTOCOL §9.3).
    let single_params: Vec<[Value; 4]> = (0..SINGLE_M).map(|i| row_params(i as u64)).collect();
    frust_shell_common::perf::mark_scenario_start("d1-insert-single");
    for (i, params) in single_params.into_iter().enumerate() {
        let t = Instant::now();
        let result = db.execute(INSERT_ROW_SQL, params);
        let us = t.elapsed().as_micros();
        single_inserts += 1;
        let err = result.is_err();
        if err {
            single_errors += 1;
        }
        frust_shell_common::perf::bench_emit(&format!(
            "frust-perf op scenario=d1 op=insert_single n={i} us={us} err={}",
            err as u8
        ));
    }
    frust_shell_common::perf::mark_scenario_end("d1-insert-single");

    let total_errors = batch_errors + single_errors;
    if total_errors > 0 {
        frust_shell_common::perf::bench_emit(&format!(
            "frust-perf plugin d1-errors insert_batch_errors={batch_errors} \
             insert_single_errors={single_errors}"
        ));
    }

    D1Report {
        batch_reps,
        batch_errors,
        single_inserts,
        single_errors,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-language golden vectors for the PROTOCOL §9.2 generator, as
    /// `(i, name, value, payload[..16] hex, payload[240..] hex)`.
    ///
    /// These are **literals on purpose**: the Flutter port asserts this
    /// exact table in `flutter_bench/test/db_scenarios_test.dart`, so a
    /// future change to either generator fails both suites instead of
    /// silently making the two columns benchmark different bytes. The
    /// payload's first and last 16 bytes pin the first and last
    /// [`splitmix64`] draws of the 32 that fill it, so any divergence in
    /// seed derivation, draw order, or byte order shows up here.
    const GOLDEN_ROWS: [(u64, &str, f64, &str, &str); 6] = [
        (
            0,
            "row-0000000000-68ae0df17b1e8e16f33806e2879f1a192fc0f9e9669731fd8",
            141.727,
            "16d09fc227e4e2551533c542b3d50565",
            "f6bac1a555538369b3d8f72f7c5fcc8a",
        ),
        (
            1,
            "row-0000000001-fa056277dff1c0f1e190da2dbfcda3cd0d6a402bf608ef0ad",
            707.948,
            "7f0c9cbda74d1d5041240093ad1af6c6",
            "657a4d65f1714d18b62c95c6f9b73741",
        ),
        (
            7,
            "row-0000000007-5930bfbe49a1003378b157c117a0aabef53fc6bdc4275b423",
            461.577,
            "9fcbd7fa58a67c677724c4623f5be1e5",
            "a0da18640a0a75404ca622515992b654",
        ),
        (
            42,
            "row-0000000042-32647b1b168a72be2b6c15067008ba70890294a8ec66378d0",
            893.322,
            "59ececb64c866902eddbd8f03a48892b",
            "a486a9d3a9c685dff6be820049868937",
        ),
        (
            1999,
            "row-0000001999-a2e1317d1982eb3940e6fb1d08960ad3b5f8458841f56f46b",
            146.531,
            "539b8392437263c1c54b66645eae9f02",
            "b2012fe1bfa2a4b448634b4dac2f9cd5",
        ),
        (
            49999,
            "row-0000049999-8b62bde346120b04252d69eef2ef1c58d42dcda2c69714584",
            617.393,
            "47e4bcc02417b53c3bfdb8c3d3145a49",
            "caa34662b313bab74f9e4ce4a6ef8b68",
        ),
    ];

    /// Lowercase hex of `bytes`, the form [`GOLDEN_ROWS`] records (and the
    /// form the Dart suite compares against).
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn generator_matches_the_cross_language_golden_vectors() {
        for (i, name, value, head_hex, tail_hex) in GOLDEN_ROWS {
            assert_eq!(row_name(ROW_SEED, i), name, "name for i={i}");
            assert_eq!(row_value(ROW_SEED, i), value, "value for i={i}");
            let payload = row_payload(ROW_SEED, i);
            assert_eq!(payload.len(), 256, "payload length for i={i}");
            assert_eq!(hex(&payload[..16]), head_hex, "payload head for i={i}");
            assert_eq!(hex(&payload[240..]), tail_hex, "payload tail for i={i}");
        }
    }

    #[test]
    fn row_name_is_exactly_64_bytes_and_deterministic() {
        let a = row_name(ROW_SEED, 7);
        let b = row_name(ROW_SEED, 7);
        assert_eq!(a.len(), 64);
        assert_eq!(a, b);
        assert_ne!(a, row_name(ROW_SEED, 8));
    }

    #[test]
    fn row_payload_is_exactly_256_bytes_and_deterministic() {
        let a = row_payload(ROW_SEED, 42);
        let b = row_payload(ROW_SEED, 42);
        assert_eq!(a.len(), 256);
        assert_eq!(a, b);
        assert_ne!(a, row_payload(ROW_SEED, 43));
    }

    #[test]
    fn row_value_is_deterministic() {
        assert_eq!(row_value(ROW_SEED, 3), row_value(ROW_SEED, 3));
    }

    #[test]
    fn scratch_cleanup_removes_the_file_on_drop() {
        let (db, path) = open_scratch_db("cleanup-test").expect("open scratch db");
        assert!(path.exists(), "scratch file should exist right after open");
        drop(db);
        {
            let _cleanup = ScratchCleanup(path.clone());
        }
        assert!(
            !path.exists(),
            "scratch file should be removed once ScratchCleanup drops"
        );
    }

    #[test]
    fn recreate_rows_table_and_insert_roundtrip_in_memory() {
        let db = Database::open_in_memory().expect("in-memory open");
        recreate_rows_table(&db).expect("create table");
        db.execute(INSERT_ROW_SQL, row_params(1))
            .expect("insert row 1");
        let rows = db.query("SELECT id, name FROM rows", ()).expect("select");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get(0), Some(&Value::Integer(1)));
    }

    #[test]
    fn recreate_rows_table_is_idempotent_across_reps() {
        // The "fresh empty table each rep" contract: a second recreate over
        // a populated table must leave it empty again, not error.
        let db = Database::open_in_memory().expect("in-memory open");
        recreate_rows_table(&db).expect("create table");
        db.execute(INSERT_ROW_SQL, row_params(1))
            .expect("insert row 1");
        recreate_rows_table(&db).expect("recreate table");
        let rows = db.query("SELECT id FROM rows", ()).expect("select");
        assert!(rows.is_empty());
    }

    #[test]
    fn run_d1_bench_smoke() {
        // A full run against a real scratch file (not just the in-memory
        // helpers above) — exercises open/run/cleanup end-to-end.
        let report = run_d1_bench();
        assert!(
            report.error.is_none(),
            "unexpected error: {:?}",
            report.error
        );
        assert_eq!(report.batch_reps, BATCH_REPS);
        assert_eq!(report.single_inserts, SINGLE_M);
        assert_eq!(report.batch_errors, 0);
        assert_eq!(report.single_errors, 0);
    }
}
