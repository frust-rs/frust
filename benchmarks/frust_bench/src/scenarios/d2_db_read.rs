//! D2 — DB read latency (`frust-database`'s point-lookup + range-scan paths
//! — `benchmarks/PROTOCOL.md` §9.4).
//!
//! Same `d*`-namespace shape as D1 ([`super::d1_db_write`]'s module doc,
//! §9.1): op-latency only, no frame series. Each run: opens a fresh scratch
//! database ([`open_scratch_db`](super::d1_db_write::open_scratch_db)),
//! pre-seeds `SEED_ROWS` rows (untimed — the seed load is never folded into
//! a `select_point`/`range_scan` `us` value), then runs `POINT_M` point
//! `select_point` reads at a fixed deterministic key permutation
//! ([`key_permutation`]) plus one `range_scan` over a fixed id range —
//! every op verified against the row the §9.2 generator deterministically
//! produces for that `id`.
//!
//! Row generation ([`row_name`]/[`row_value`]/[`row_payload`]) is owned by
//! [`super::d1_db_write`] and imported here rather than duplicated — see
//! that module's doc for the shared-dataset rationale.

use std::hint::black_box;
use std::time::Instant;

use frust::{
    Align, Alignment, AnyView, AsyncValue, Component, Get, UseTask, View, any, component, text,
    use_task,
};
use frust_database::{Database, Row, Value};

use super::d1_db_write::{
    INSERT_ROW_SQL, ROW_SEED, open_scratch_db, recreate_rows_table, row_name, row_params,
    row_payload, row_value, splitmix64,
};
use super::{BenchState, Scenario};

/// Rows pre-seeded before the timed phase (PROTOCOL §9.4, declared
/// convention).
const SEED_ROWS: u64 = 50_000;
/// Rows per untimed pre-seed batch transaction — an implementation detail
/// (PROTOCOL §9.4 only constrains that the seed load itself is never timed),
/// chosen for reasonable pre-seed wall time.
const SEED_BATCH: u64 = 5_000;
/// Point `select_point` ops per run (PROTOCOL §9.4).
const POINT_M: usize = 500;
/// `range_scan`'s fixed id range: `id BETWEEN RANGE_LO AND RANGE_HI`
/// (`RANGE_HI - RANGE_LO + 1 == RANGE_SPAN == 5,000` — PROTOCOL §9.4).
const RANGE_LO: u64 = 10_000;
const RANGE_HI: u64 = 14_999;
const RANGE_SPAN: u64 = RANGE_HI - RANGE_LO + 1;

/// D2 — DB read latency.
pub struct D2;

impl Scenario for D2 {
    fn id(&self) -> &'static str {
        "d2"
    }

    fn title(&self) -> &'static str {
        "DB reads (frust-database)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        any(component(D2Read))
    }
}

/// Deterministic permutation of `0..n`, seeded by [`ROW_SEED`] via the same
/// [`splitmix64`] generator [`row_name`]/[`row_value`]/[`row_payload`] use —
/// PROTOCOL §9.4's "fixed deterministic permutation... same per-index draw
/// order on both apps" (a Fisher-Yates shuffle, so nothing here depends on
/// wall-clock or process-local RNG state). The Flutter side ports this
/// exact draw sequence — `nextU64() % (i + 1)` per swap, unscaled — and
/// both suites assert the same literal golden head (see `tests`).
fn key_permutation(n: u64) -> Vec<u64> {
    let mut perm: Vec<u64> = (0..n).collect();
    let mut state = ROW_SEED ^ 0x4444;
    for i in (1..perm.len()).rev() {
        let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
        perm.swap(i, j);
    }
    perm
}

/// The outcome of verifying one read against the §9.2 generator's expected
/// row for that `id` — mirrors S8's `ReadCheck` split.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReadCheck {
    Match,
    UnexpectedNone,
    ValueMismatch,
}

/// Whether `row`'s `(name, value, payload)` columns match the §9.2
/// generator's expected values for `id`.
fn row_matches_expected(row: &Row, id: u64) -> bool {
    let expected_name = Value::Text(row_name(ROW_SEED, id));
    let expected_value = row_value(ROW_SEED, id);
    let expected_payload = Value::Blob(row_payload(ROW_SEED, id));
    let name_ok = row.get_named("name") == Some(&expected_name);
    let value_ok = matches!(row.get_named("value"), Some(Value::Real(v)) if *v == expected_value);
    let payload_ok = row.get_named("payload") == Some(&expected_payload);
    name_ok && value_ok && payload_ok
}

/// The outcome of one D2 run, surfaced as the status readout.
#[derive(Clone)]
pub struct D2Report {
    point_reads: usize,
    point_errors: usize,
    range_rows: usize,
    range_error: bool,
    /// Set if opening the scratch database or the untimed pre-seed itself
    /// failed — D2 then reports zero ops rather than panicking, mirroring
    /// S8/D1's graceful-error contract.
    error: Option<String>,
}

/// D2 — DB read latency component.
pub struct D2Read;

/// Retained D2 state: just the run task.
pub struct D2State {
    task: UseTask<D2Report>,
}

impl Component for D2Read {
    type State = D2State;

    fn init(&self) -> D2State {
        let task = use_task(|| async { frust::spawn_blocking(run_d2_bench).await });
        D2State { task }
    }

    fn build(&self, state: &mut D2State) -> impl View<D2State> {
        let status = match state.task.signal().get() {
            AsyncValue::Idle | AsyncValue::Loading(_) => format!(
                "seeding {SEED_ROWS} rows then running D2 read bench \
                 ({POINT_M} point reads + 1 range scan)…"
            ),
            AsyncValue::Ready(report) => match report.error {
                Some(msg) => format!("D2 backend unavailable: {msg}"),
                None => {
                    let errors = report.point_errors + report.range_error as usize;
                    format!(
                        "D2 complete — {} point reads, range scan matched {} row(s){}",
                        report.point_reads,
                        report.range_rows,
                        if errors > 0 {
                            format!(
                                " — {errors} error(s): point={} range={}",
                                report.point_errors, report.range_error as usize
                            )
                        } else {
                            String::new()
                        },
                    )
                }
            },
            AsyncValue::Error(_) => "D2 run task failed".to_string(),
        };

        any(Align(Alignment::new(0.0, 0.0), text(status).size(18.0)))
    }
}

/// Load [`SEED_ROWS`] rows via [`row_params`], batched into
/// [`SEED_BATCH`]-row transactions — untimed setup, never folded into a
/// `select_point`/`range_scan` `us` value.
fn seed_rows(db: &Database) -> Result<(), frust_database::DatabaseError> {
    recreate_rows_table(db)?;
    let mut start = 0u64;
    while start < SEED_ROWS {
        let end = (start + SEED_BATCH).min(SEED_ROWS);
        db.transaction(|txn| {
            for id in start..end {
                txn.execute(INSERT_ROW_SQL, row_params(id))?;
            }
            Ok(())
        })?;
        start = end;
    }
    Ok(())
}

/// The measured run (runs off the UI thread). Opens a fresh scratch
/// database, pre-seeds it, runs both read phases in PROTOCOL §9.4's declared
/// order, logs every op as a canonical per-op line (§7), and brackets each
/// phase with a marker pair for the phase's total wall time.
fn run_d2_bench() -> D2Report {
    let (db, path) = match open_scratch_db("d2") {
        Ok(v) => v,
        Err(e) => {
            return D2Report {
                point_reads: 0,
                point_errors: 0,
                range_rows: 0,
                range_error: false,
                error: Some(e.to_string()),
            };
        }
    };
    let _cleanup = super::d1_db_write::ScratchCleanup(path);

    if let Err(e) = seed_rows(&db) {
        return D2Report {
            point_reads: 0,
            point_errors: 0,
            range_rows: 0,
            range_error: false,
            error: Some(format!("pre-seed failed: {e}")),
        };
    }

    // --- POINT_M × select_point, fixed deterministic key permutation ---
    let keys = key_permutation(SEED_ROWS);
    frust_shell_common::perf::mark_scenario_start("d2-select-point");
    let mut point_reads = 0;
    let mut point_errors = 0;
    for (n, &id) in keys.iter().take(POINT_M).enumerate() {
        let t = Instant::now();
        let result = db.query(
            "SELECT id, name, value, payload FROM rows WHERE id = ?1",
            [Value::Integer(id as i64)],
        );
        let us = t.elapsed().as_micros();
        point_reads += 1;
        let check = match black_box(&result) {
            Ok(rows) if rows.len() == 1 && row_matches_expected(&rows[0], id) => ReadCheck::Match,
            Ok(rows) if rows.is_empty() => ReadCheck::UnexpectedNone,
            Ok(_) => ReadCheck::ValueMismatch,
            Err(_) => ReadCheck::ValueMismatch,
        };
        let err = check != ReadCheck::Match;
        if err {
            point_errors += 1;
        }
        frust_shell_common::perf::bench_emit(&format!(
            "frust-perf op scenario=d2 op=select_point n={n} us={us} err={}",
            err as u8
        ));
    }
    frust_shell_common::perf::mark_scenario_end("d2-select-point");

    // --- one range_scan, WHERE id BETWEEN RANGE_LO AND RANGE_HI ---
    frust_shell_common::perf::mark_scenario_start("d2-range-scan");
    let t = Instant::now();
    let result = db.query(
        "SELECT id, name, value, payload FROM rows WHERE id BETWEEN ?1 AND ?2 ORDER BY id",
        [
            Value::Integer(RANGE_LO as i64),
            Value::Integer(RANGE_HI as i64),
        ],
    );
    let us = t.elapsed().as_micros();
    let (range_rows, range_error) = match black_box(&result) {
        Ok(rows) => {
            let count_ok = rows.len() as u64 == RANGE_SPAN;
            let first_ok = rows
                .first()
                .is_some_and(|r| row_matches_expected(r, RANGE_LO));
            let last_ok = rows
                .last()
                .is_some_and(|r| row_matches_expected(r, RANGE_HI));
            (rows.len(), !(count_ok && first_ok && last_ok))
        }
        Err(_) => (0, true),
    };
    frust_shell_common::perf::bench_emit(&format!(
        "frust-perf op scenario=d2 op=range_scan n=0 us={us} err={} rows={range_rows}",
        range_error as u8
    ));
    frust_shell_common::perf::mark_scenario_end("d2-range-scan");

    let total_errors = point_errors + range_error as usize;
    if total_errors > 0 {
        frust_shell_common::perf::bench_emit(&format!(
            "frust-perf plugin d2-errors select_point_errors={point_errors} \
             range_scan_errors={}",
            range_error as usize
        ));
    }

    D2Report {
        point_reads,
        point_errors,
        range_rows,
        range_error,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first ten keys `key_permutation(SEED_ROWS)` yields — a
    /// cross-language golden vector, asserted as literals here and again, as
    /// the same literals, in
    /// `flutter_bench/test/db_scenarios_test.dart`. PROTOCOL §9.4 requires
    /// both apps to hit the identical key sequence; this is what makes a
    /// divergence fail a CI run rather than skew a published latency.
    const GOLDEN_PERMUTATION_HEAD: [u64; 10] = [
        38108, 16571, 44726, 49398, 6580, 17293, 11601, 47409, 1854, 49530,
    ];

    #[test]
    fn key_permutation_matches_the_cross_language_golden_vector() {
        let perm = key_permutation(SEED_ROWS);
        assert_eq!(perm[..10], GOLDEN_PERMUTATION_HEAD);
    }

    #[test]
    fn key_permutation_is_deterministic_and_a_bijection() {
        let a = key_permutation(1000);
        let b = key_permutation(1000);
        assert_eq!(a, b);
        let mut sorted = a.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..1000).collect::<Vec<u64>>());
    }

    #[test]
    fn row_matches_expected_detects_mismatch() {
        let db = Database::open_in_memory().expect("in-memory open");
        recreate_rows_table(&db).expect("create table");
        db.execute(INSERT_ROW_SQL, row_params(5)).expect("insert");
        let rows = db
            .query("SELECT id, name, value, payload FROM rows WHERE id = 5", ())
            .expect("select");
        assert!(row_matches_expected(&rows[0], 5));
        assert!(!row_matches_expected(&rows[0], 6));
    }

    #[test]
    fn run_d2_bench_smoke() {
        // A full (if small-scale) run against a real scratch file, seed
        // included — exercises open/seed/cleanup end-to-end.
        let report = run_d2_bench();
        assert!(
            report.error.is_none(),
            "unexpected error: {:?}",
            report.error
        );
        assert_eq!(report.point_reads, POINT_M);
        assert_eq!(report.point_errors, 0);
        assert_eq!(report.range_rows, RANGE_SPAN as usize);
        assert!(!report.range_error);
    }
}
