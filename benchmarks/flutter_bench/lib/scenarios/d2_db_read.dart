/// d2 — DB reads. Point `SELECT`s by primary key over a pre-seeded table,
/// repeated, plus one range scan — the point-lookup-latency +
/// sequential-scan-throughput workload (PROTOCOL §9.4). Runs against
/// whichever [DbAdapter] `DB_ADAPTER` selects (`db_adapter.dart`), sharing the
/// row-shape generator, table schema, and per-op line formatter with
/// `d1_db_write.dart` (imported from there, per that file's doc comment).
///
/// **Per-run sequence** (PROTOCOL §9.4): untimed pre-seed of [dSeedRows] rows
/// → [dPointM] × `select_point` (key drawn from [dPointReadPermutation]) →
/// one `range_scan` (`id BETWEEN [dRangeLow] AND [dRangeHigh]`, fully
/// iterated).
///
/// Nothing the §9.2 generator produces is inside a timed window here: the
/// pre-seed and the key permutation are built before the timed phase, and
/// each op's expected-row verification runs after its [Stopwatch] stops, so
/// a `us` value is the query alone (§9.2's generation-outside-the-timed-
/// window rule).
///
/// Per-op lines:
/// ```
/// flutter-perf op scenario=d2 op=select_point n=<u64> us=<u64> err=<0|1>
/// flutter-perf op scenario=d2 op=range_scan n=<u64> us=<u64> err=<0|1> rows=<matched-row-count>
/// ```
library;

import 'package:flutter/material.dart';

import '../bench/perf.dart';
import 'd1_db_write.dart';
import 'db_adapter.dart';

class DbReadView extends StatefulWidget {
  const DbReadView({super.key});

  @override
  State<DbReadView> createState() => _DbReadViewState();
}

class _DbReadViewState extends State<DbReadView> {
  String _status = 'starting…';

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _run());
  }

  Future<void> _run() async {
    final adapter = selectDbAdapter();
    final dbPath = await dScenarioDbPath('d2', adapter.name);
    await adapter.open(dbPath);

    benchEmit(formatDbInfoLine(
      scenario: 'd2',
      adapter: adapter.name,
      sqliteVersion: await adapter.engineVersion(),
    ));

    // --- untimed pre-seed: dSeedRows rows, one batched transaction so the
    // seed load itself is fast and never folded into a select_point/
    // range_scan `us` value ---
    await dResetTable(adapter);
    await adapter.transaction((txn) async {
      for (var i = 0; i < dSeedRows; i++) {
        await dInsertRow(txn, i);
      }
    });

    var pointErrors = 0;
    var rangeErrors = 0;

    // --- select_point: dPointM point reads, key sequence = the fixed
    // deterministic permutation's first dPointM entries ---
    final keys = dPointReadPermutation();
    markScenarioStart('d2-select-point');
    for (var i = 0; i < dPointM; i++) {
      final id = keys[i];
      List<Map<String, Object?>> rows = const [];
      var err = false;
      final sw = Stopwatch()..start();
      try {
        rows = await adapter.query(
          'SELECT id, name, value, payload FROM bench_rows WHERE id = ?',
          [id],
        );
      } catch (_) {
        err = true;
      }
      sw.stop();
      if (!err && !_rowMatchesExpected(rows, id)) err = true;
      if (err) pointErrors++;
      benchEmit(formatDbOpLine(
        scenario: 'd2',
        op: 'select_point',
        n: i,
        us: sw.elapsedMicroseconds,
        err: err,
      ));
    }
    markScenarioEnd('d2-select-point');

    // --- range_scan: one op, fully iterated, id BETWEEN dRangeLow AND
    // dRangeHigh ---
    markScenarioStart('d2-range-scan');
    List<Map<String, Object?>> rangeRows = const [];
    var rangeErr = false;
    final rangeSw = Stopwatch()..start();
    try {
      rangeRows = await adapter.query(
        'SELECT id, name, value, payload FROM bench_rows '
        'WHERE id BETWEEN ? AND ? ORDER BY id',
        [dRangeLow, dRangeHigh],
      );
    } catch (_) {
      rangeErr = true;
    }
    rangeSw.stop();
    final expectedRangeCount = dRangeHigh - dRangeLow + 1;
    if (!rangeErr) {
      if (rangeRows.length != expectedRangeCount ||
          !_rowMatchesExpected([rangeRows.first], dRangeLow) ||
          !_rowMatchesExpected([rangeRows.last], dRangeHigh)) {
        rangeErr = true;
      }
    }
    if (rangeErr) rangeErrors++;
    benchEmit(formatDbOpLine(
      scenario: 'd2',
      op: 'range_scan',
      n: 0,
      us: rangeSw.elapsedMicroseconds,
      err: rangeErr,
      extra: {'rows': rangeRows.length},
    ));
    markScenarioEnd('d2-range-scan');

    if (pointErrors > 0 || rangeErrors > 0) {
      benchEmit(formatDbErrorsLine('d2', {
        'select_point_errors': pointErrors,
        'range_scan_errors': rangeErrors,
      }));
    }

    await adapter.close();

    if (!mounted) return;
    setState(() => _status = 'd2 complete (adapter=${adapter.name})'
        '${pointErrors + rangeErrors > 0 ? ' — ${pointErrors + rangeErrors} '
            'error(s): point=$pointErrors range=$rangeErrors' : ''}');
  }

  /// Verifies a `select_point`/`range_scan` row against the deterministic
  /// value the §9.2 generator produces for [id] — a mismatch or
  /// unexpected-empty result is what sets `err=1` (PROTOCOL §9.4, mirroring
  /// S8's read-verification split).
  bool _rowMatchesExpected(List<Map<String, Object?>> rows, int id) {
    if (rows.isEmpty) return false;
    final row = rows.first;
    if (row['id'] != id) return false;
    if (row['name'] != dRowName(id)) return false;
    final value = row['value'];
    if (value is! num || (value - dRowValue(id)).abs() > 1e-9) return false;
    final payload = row['payload'];
    final expectedPayload = dRowPayload(id);
    if (payload is! List || payload.length != expectedPayload.length) {
      return false;
    }
    for (var i = 0; i < payload.length; i++) {
      if (payload[i] != expectedPayload[i]) return false;
    }
    return true;
  }

  @override
  Widget build(BuildContext context) {
    return Center(child: Text(_status));
  }
}
