/// Tests for the `d1`/`d2` DB-scenario building blocks: the PROTOCOL §7
/// canonical per-op line formatter (pure, directly testable — mirrors
/// `trace_format_test.dart`'s raw-frame-line test), and [Sqlite3Adapter]'s
/// param binding, which runs on host since `package:sqlite3` is in-process
/// FFI with no platform channel.
library;

import 'package:flutter_bench/scenarios/d1_db_write.dart';
import 'package:flutter_bench/scenarios/db_adapter.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('formatDbOpLine', () {
    test('formats the canonical shape with no extra keys', () {
      expect(
        formatDbOpLine(
            scenario: 'd2', op: 'select_point', n: 7, us: 123, err: false),
        'flutter-perf op scenario=d2 op=select_point n=7 us=123 err=0',
      );
    });

    test('formats err=1 and appends extra keys in order', () {
      expect(
        formatDbOpLine(
          scenario: 'd1',
          op: 'insert_batch',
          n: 0,
          us: 45678,
          err: true,
          extra: {'rows': 2000},
        ),
        'flutter-perf op scenario=d1 op=insert_batch n=0 us=45678 err=1 '
        'rows=2000',
      );
    });

    test('round-trips through a key=value parse', () {
      final line = formatDbOpLine(
        scenario: 'd2',
        op: 'range_scan',
        n: 0,
        us: 9999,
        err: false,
        extra: {'rows': 5000},
      );
      final fields = <String, String>{};
      for (final part in line.split(' ').skip(2)) {
        final kv = part.split('=');
        if (kv.length == 2) fields[kv[0]] = kv[1];
      }
      expect(fields['scenario'], 'd2');
      expect(fields['op'], 'range_scan');
      expect(fields['n'], '0');
      expect(fields['us'], '9999');
      expect(fields['err'], '0');
      expect(fields['rows'], '5000');
    });
  });

  group('formatDbErrorsLine', () {
    test('formats the per-run failure-tally marker', () {
      expect(
        formatDbErrorsLine(
            'd1', {'insert_batch_errors': 1, 'insert_single_errors': 0}),
        'flutter-perf plugin d1-errors insert_batch_errors=1 '
        'insert_single_errors=0',
      );
    });
  });

  group('formatDbInfoLine', () {
    test('formats the one-shot version-info line', () {
      expect(
        formatDbInfoLine(
            scenario: 'd1', adapter: 'ffi', sqliteVersion: '3.53.4'),
        'flutter-perf info scenario=d1 adapter=ffi sqlite_version=3.53.4',
      );
    });
  });

  group('row-shape generator (PROTOCOL §9.2)', () {
    test('is deterministic for a given index', () {
      expect(dRowName(5), dRowName(5));
      expect(dRowValue(5), dRowValue(5));
      expect(dRowPayload(5), dRowPayload(5));
    });

    test('name is exactly 64 bytes and payload is exactly 256 bytes', () {
      expect(dRowName(0).length, 64);
      expect(dRowName(12345).length, 64);
      expect(dRowPayload(0).length, 256);
    });

    test('distinct indices produce distinct rows', () {
      expect(dRowName(1), isNot(dRowName(2)));
      expect(dRowValue(1), isNot(dRowValue(2)));
      expect(dRowPayload(1), isNot(dRowPayload(2)));
    });
  });

  group('dPointReadPermutation', () {
    test('is a permutation of 0..dSeedRows and deterministic', () {
      final a = dPointReadPermutation();
      final b = dPointReadPermutation();
      expect(a, b);
      expect(a.length, dSeedRows);
      expect(a.toSet().length, dSeedRows);
      expect(a.reduce((x, y) => x > y ? x : y), dSeedRows - 1);
      expect(a.reduce((x, y) => x < y ? x : y), 0);
    });
  });

  group('Sqlite3Adapter param binding (host-runnable, in-process FFI)', () {
    late Sqlite3Adapter adapter;

    setUp(() async {
      adapter = Sqlite3Adapter();
      await adapter.open(':memory:');
    });

    tearDown(() async {
      await adapter.close();
    });

    test('binds positional ? params on execute/query round-trip', () async {
      await adapter.execute(dCreateTableSql);
      await adapter.execute(
        'INSERT INTO bench_rows (id, name, value, payload) '
        'VALUES (?, ?, ?, ?)',
        [1, 'hello', 3.5, [1, 2, 3]],
      );
      final rows =
          await adapter.query('SELECT * FROM bench_rows WHERE id = ?', [1]);
      expect(rows, hasLength(1));
      expect(rows.first['id'], 1);
      expect(rows.first['name'], 'hello');
      expect(rows.first['value'], 3.5);
      expect(rows.first['payload'], [1, 2, 3]);
    });

    test('a mismatched id binding returns no rows', () async {
      await adapter.execute(dCreateTableSql);
      await adapter.execute(
        'INSERT INTO bench_rows (id, name, value, payload) '
        'VALUES (?, ?, ?, ?)',
        [1, 'hello', 3.5, [1, 2, 3]],
      );
      final rows =
          await adapter.query('SELECT * FROM bench_rows WHERE id = ?', [2]);
      expect(rows, isEmpty);
    });

    test('transaction commits all writes issued on the txn handle', () async {
      await adapter.execute(dCreateTableSql);
      await adapter.transaction((txn) async {
        for (var i = 0; i < 5; i++) {
          await dInsertRow(txn, i);
        }
      });
      final rows = await adapter.query('SELECT id FROM bench_rows');
      expect(rows, hasLength(5));
    });

    test('a thrown exception inside transaction rolls back', () async {
      await adapter.execute(dCreateTableSql);
      await expectLater(
        adapter.transaction((txn) async {
          await dInsertRow(txn, 0);
          throw StateError('boom');
        }),
        throwsStateError,
      );
      final rows = await adapter.query('SELECT id FROM bench_rows');
      expect(rows, isEmpty);
    });

    test('engineVersion reports package:sqlite3\'s linked version', () async {
      final v = await adapter.engineVersion();
      expect(v, matches(RegExp(r'^\d+\.\d+\.\d+$')));
    });

    test('name is ffi', () {
      expect(adapter.name, 'ffi');
    });
  });
}
