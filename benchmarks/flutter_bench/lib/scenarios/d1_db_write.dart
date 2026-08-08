/// d1 — DB writes. An `N`-row batched insert inside one transaction,
/// repeated, plus `M` single-row autocommit inserts, repeated — the
/// transactional-batch-throughput-vs-per-call-autocommit-overhead workload
/// (PROTOCOL §9.3). Runs against whichever [DbAdapter] `DB_ADAPTER` selects
/// (`db_adapter.dart`), so `d1`'s own code never branches on which SQLite
/// story is behind it.
///
/// This file also carries the row-shape/seed-dataset generator (PROTOCOL
/// §9.2) and the PROTOCOL §7 canonical per-op line formatter, both shared
/// with `d2_db_read.dart` (which imports this file for them) — kept here
/// rather than in `bench/datasets.dart` to keep this task's diff to the
/// files it was scoped to touch.
///
/// **Per-run sequence** (PROTOCOL §9.3): drop/recreate table → [dBatchReps] ×
/// `insert_batch` (fresh empty table each rep) → drop/recreate table once →
/// [dSingleM] × `insert_single`.
///
/// Per-op lines (PROTOCOL §7's canonical `op`-token / inline-`scenario=`
/// shape — d1/d2 are the first scenarios to speak it; S8 predates it and
/// keeps its own grandfathered `plugin`-token shape):
/// ```
/// flutter-perf op scenario=d1 op=insert_batch n=<u64> us=<u64> err=<0|1> rows=<BATCH_N>
/// flutter-perf op scenario=d1 op=insert_single n=<u64> us=<u64> err=<0|1>
/// ```
library;

import 'package:flutter/material.dart';

import '../bench/perf.dart';
import '../bench/rng.dart';
import 'db_adapter.dart';

// ---------------------------------------------------------------------------
// Row shape + seed dataset (PROTOCOL §9.2, shared by d1 and d2)
// ---------------------------------------------------------------------------

/// Deterministic seed for the d1/d2 row generator (PROTOCOL §9.2 — an
/// arbitrary fixed constant; no external standard prescribes a benchmark
/// seed).
const int dRowSeed = 424242;

/// d1 sizing (PROTOCOL §9.3): rows per `insert_batch` transaction.
const int dBatchN = 2000;

/// d1 sizing: `insert_batch` reps per run (fresh empty table each rep).
const int dBatchReps = 10;

/// d1 sizing: single-row autocommit `insert_single` ops per run.
const int dSingleM = 500;

/// d2 sizing (PROTOCOL §9.4): rows pre-seeded before the timed phase.
const int dSeedRows = 50000;

/// d2 sizing: point `select_point` ops per run.
const int dPointM = 500;

/// d2 sizing: the range scan's inclusive lower bound (`id BETWEEN
/// [dRangeLow] AND [dRangeHigh]`).
const int dRangeLow = 10000;

/// d2 sizing: the range scan's inclusive upper bound.
const int dRangeHigh = 14999;

const String _hexDigits = '0123456789abcdef';

/// Row `i`'s deterministic `name` (~64 bytes) — a pure function of
/// `(dRowSeed, i)`, no wall-clock or process-local RNG state, so the frust
/// side's mirrored generator reproduces the same value for the same `i`
/// (PROTOCOL §9.2's "identical logical work" fairness gate).
String dRowName(int i) {
  final rng = SplitMix64(dRowSeed + i * 4 + 1);
  final sb = StringBuffer('row_')..write(i)..write('_');
  while (sb.length < 64) {
    sb.write(_hexDigits[rng.nextU64() & 0xF]);
  }
  return sb.toString().substring(0, 64);
}

/// Row `i`'s deterministic `value` (REAL) — a pure function of `(dRowSeed,
/// i)`.
double dRowValue(int i) {
  final rng = SplitMix64(dRowSeed + i * 4 + 2);
  return rng.nextF64() * 1000.0;
}

/// Row `i`'s deterministic ~256-byte `payload` (BLOB) — a pure function of
/// `(dRowSeed, i)`.
List<int> dRowPayload(int i) {
  final rng = SplitMix64(dRowSeed + i * 4 + 3);
  return [for (var j = 0; j < 256; j++) rng.nextByte()];
}

/// A fixed deterministic permutation of `0..dSeedRows` for d2's point-read
/// key sequence (PROTOCOL §9.4: "a fixed deterministic permutation of
/// `0..SEED_ROWS` (seed = 424242, same per-index draw order on both apps)")
/// — a Fisher-Yates shuffle seeded by [dRowSeed]. Only the first [dPointM]
/// entries are ever consumed by d2, but the full permutation is produced so
/// every index is equally likely to appear in that prefix.
List<int> dPointReadPermutation() {
  final ids = List<int>.generate(dSeedRows, (i) => i);
  final rng = SplitMix64(dRowSeed + 4 * dSeedRows);
  for (var i = ids.length - 1; i > 0; i--) {
    final j = rng.nextIntBelow(i + 1);
    final tmp = ids[i];
    ids[i] = ids[j];
    ids[j] = tmp;
  }
  return ids;
}

// ---------------------------------------------------------------------------
// Schema + table reset — shared by d1 (writes) and d2 (reads).
// ---------------------------------------------------------------------------

/// The fixed row shape both d1 and d2 use (PROTOCOL §9.2).
const String dCreateTableSql = '''
CREATE TABLE IF NOT EXISTS bench_rows (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  value REAL NOT NULL,
  payload BLOB NOT NULL
)''';

/// Drops and recreates `bench_rows` — used before each d1 `insert_batch` rep
/// (fresh empty table so batch cost is never inflated by a growing table
/// across reps, PROTOCOL §9.3) and once before d1's `insert_single` phase /
/// d2's pre-seed phase.
Future<void> dResetTable(DbAdapter db) async {
  await db.execute('DROP TABLE IF EXISTS bench_rows');
  await db.execute(dCreateTableSql);
}

const String _insertSql =
    'INSERT INTO bench_rows (id, name, value, payload) VALUES (?, ?, ?, ?)';

/// Inserts row `i` (PROTOCOL §9.2's generator) via [db] — the one INSERT
/// statement issued, identically, by every phase of d1 and by d2's pre-seed,
/// on both adapters.
Future<void> dInsertRow(DbAdapter db, int i) => db.execute(
      _insertSql,
      [i, dRowName(i), dRowValue(i), dRowPayload(i)],
    );

// ---------------------------------------------------------------------------
// PROTOCOL §7 canonical per-op line format — shared by d1 and d2.
// ---------------------------------------------------------------------------

/// Formats one PROTOCOL §7 canonical per-op line — the `op`-token /
/// inline-`scenario=` shape every per-op-latency scenario starting with d1/d2
/// speaks (distinct from S8's grandfathered `plugin`-token lines, which
/// PROTOCOL §7 leaves as-is). Pure and directly unit-testable.
///
/// `extra` keys are appended, in iteration order, after the five canonical
/// fields (e.g. d1's `rows=` on `insert_batch`, d2's `rows=` on
/// `range_scan`).
String formatDbOpLine({
  required String scenario,
  required String op,
  required int n,
  required int us,
  required bool err,
  Map<String, Object> extra = const {},
}) {
  final buf = StringBuffer()
    ..write('flutter-perf op scenario=$scenario op=$op n=$n us=$us '
        'err=${err ? 1 : 0}');
  extra.forEach((k, v) => buf.write(' $k=$v'));
  return buf.toString();
}

/// Formats the per-run failure-tally marker (PROTOCOL §7): one line, emitted
/// only when at least one counter is nonzero, matching S8's `s8-errors`
/// shape.
String formatDbErrorsLine(String scenario, Map<String, int> counts) {
  final buf = StringBuffer('flutter-perf plugin $scenario-errors');
  counts.forEach((k, v) => buf.write(' $k=$v'));
  return buf.toString();
}

/// Formats the one-shot adapter/engine-version info line emitted at scenario
/// start (PROTOCOL §9.7: "Each side's exact SQLite version actually linked is
/// recorded per run").
String formatDbInfoLine({
  required String scenario,
  required String adapter,
  required String sqliteVersion,
}) =>
    'flutter-perf info scenario=$scenario adapter=$adapter '
    'sqlite_version=$sqliteVersion';

// ---------------------------------------------------------------------------
// d1 scenario widget
// ---------------------------------------------------------------------------

class DbWriteView extends StatefulWidget {
  const DbWriteView({super.key});

  @override
  State<DbWriteView> createState() => _DbWriteViewState();
}

class _DbWriteViewState extends State<DbWriteView> {
  String _status = 'starting…';

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _run());
  }

  Future<void> _run() async {
    final adapter = selectDbAdapter();
    final dbPath = await dScenarioDbPath('d1', adapter.name);
    await adapter.open(dbPath);

    benchEmit(formatDbInfoLine(
      scenario: 'd1',
      adapter: adapter.name,
      sqliteVersion: await adapter.engineVersion(),
    ));

    var batchErrors = 0;
    var singleErrors = 0;

    // --- insert_batch: BATCH_REPS reps, each a fresh table + one
    // transaction of BATCH_N rows ---
    markScenarioStart('d1-insert-batch');
    for (var rep = 0; rep < dBatchReps; rep++) {
      await dResetTable(adapter);
      var err = false;
      final sw = Stopwatch()..start();
      try {
        await adapter.transaction((txn) async {
          for (var i = 0; i < dBatchN; i++) {
            await dInsertRow(txn, i);
          }
        });
      } catch (_) {
        err = true;
      }
      sw.stop();
      if (err) batchErrors++;
      benchEmit(formatDbOpLine(
        scenario: 'd1',
        op: 'insert_batch',
        n: rep,
        us: sw.elapsedMicroseconds,
        err: err,
        extra: {'rows': dBatchN},
      ));
    }
    markScenarioEnd('d1-insert-batch');

    // --- insert_single: SINGLE_M single-row autocommit inserts into a table
    // dropped-and-recreated once ---
    await dResetTable(adapter);
    markScenarioStart('d1-insert-single');
    for (var i = 0; i < dSingleM; i++) {
      var err = false;
      final sw = Stopwatch()..start();
      try {
        await dInsertRow(adapter, i);
      } catch (_) {
        err = true;
      }
      sw.stop();
      if (err) singleErrors++;
      benchEmit(formatDbOpLine(
        scenario: 'd1',
        op: 'insert_single',
        n: i,
        us: sw.elapsedMicroseconds,
        err: err,
      ));
    }
    markScenarioEnd('d1-insert-single');

    if (batchErrors > 0 || singleErrors > 0) {
      benchEmit(formatDbErrorsLine('d1', {
        'insert_batch_errors': batchErrors,
        'insert_single_errors': singleErrors,
      }));
    }

    await adapter.close();

    if (!mounted) return;
    setState(() => _status = 'd1 complete (adapter=${adapter.name})'
        '${batchErrors + singleErrors > 0 ? ' — ${batchErrors + singleErrors} '
            'error(s): batch=$batchErrors single=$singleErrors' : ''}');
  }

  @override
  Widget build(BuildContext context) {
    return Center(child: Text(_status));
  }
}
