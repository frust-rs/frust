/// Tests for the `d1`/`d2` DB-scenario building blocks: the PROTOCOL §7
/// canonical per-op line formatter (pure, directly testable — mirrors
/// `trace_format_test.dart`'s raw-frame-line test), the PROTOCOL §9.2
/// generator's cross-language golden vectors, and [Sqlite3Adapter]'s param
/// binding and storage config, which run on host since `package:sqlite3` is
/// in-process FFI with no platform channel.
library;

import 'dart:io';

import 'package:flutter_bench/scenarios/d1_db_write.dart';
import 'package:flutter_bench/scenarios/db_adapter.dart';
import 'package:flutter_test/flutter_test.dart';

/// Cross-language golden vectors for the PROTOCOL §9.2 generator:
/// `(i, name, value, payload[0..16] hex, payload[240..256] hex)`.
///
/// These are **literals on purpose** and are the same literals the canonical
/// Rust side asserts in `frust_bench/src/scenarios/d1_db_write.rs`'s
/// `GOLDEN_ROWS`. A change to either generator therefore fails both suites,
/// instead of silently making the two benchmark columns insert different
/// bytes (§9.2's byte-identical-rows fairness gate).
const List<(int, String, double, String, String)> goldenRows = [
  (
    0,
    'row-0000000000-68ae0df17b1e8e16f33806e2879f1a192fc0f9e9669731fd8',
    141.727,
    '16d09fc227e4e2551533c542b3d50565',
    'f6bac1a555538369b3d8f72f7c5fcc8a',
  ),
  (
    1,
    'row-0000000001-fa056277dff1c0f1e190da2dbfcda3cd0d6a402bf608ef0ad',
    707.948,
    '7f0c9cbda74d1d5041240093ad1af6c6',
    '657a4d65f1714d18b62c95c6f9b73741',
  ),
  (
    7,
    'row-0000000007-5930bfbe49a1003378b157c117a0aabef53fc6bdc4275b423',
    461.577,
    '9fcbd7fa58a67c677724c4623f5be1e5',
    'a0da18640a0a75404ca622515992b654',
  ),
  (
    42,
    'row-0000000042-32647b1b168a72be2b6c15067008ba70890294a8ec66378d0',
    893.322,
    '59ececb64c866902eddbd8f03a48892b',
    'a486a9d3a9c685dff6be820049868937',
  ),
  (
    1999,
    'row-0000001999-a2e1317d1982eb3940e6fb1d08960ad3b5f8458841f56f46b',
    146.531,
    '539b8392437263c1c54b66645eae9f02',
    'b2012fe1bfa2a4b448634b4dac2f9cd5',
  ),
  (
    49999,
    'row-0000049999-8b62bde346120b04252d69eef2ef1c58d42dcda2c69714584',
    617.393,
    '47e4bcc02417b53c3bfdb8c3d3145a49',
    'caa34662b313bab74f9e4ce4a6ef8b68',
  ),
];

/// The first ten keys [dPointReadPermutation] yields — the same literals the
/// frust side asserts as `GOLDEN_PERMUTATION_HEAD`, so both apps provably hit
/// the identical point-read key sequence (PROTOCOL §9.4).
const List<int> goldenPermutationHead = [
  38108,
  16571,
  44726,
  49398,
  6580,
  17293,
  11601,
  47409,
  1854,
  49530,
];

/// Lowercase hex of [bytes] — the form [goldenRows] records.
String hex(List<int> bytes) =>
    bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join();

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
    test('matches the cross-language golden vectors', () {
      for (final (i, name, value, headHex, tailHex) in goldenRows) {
        expect(dRowName(i), name, reason: 'name for i=$i');
        expect(dRowValue(i), value, reason: 'value for i=$i');
        final payload = dRowPayload(i);
        expect(payload, hasLength(256), reason: 'payload length for i=$i');
        expect(hex(payload.sublist(0, 16)), headHex,
            reason: 'payload head for i=$i');
        expect(hex(payload.sublist(240)), tailHex,
            reason: 'payload tail for i=$i');
      }
    });

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
    test('matches the cross-language golden vector', () {
      expect(dPointReadPermutation().sublist(0, 10), goldenPermutationHead);
    });

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

  group('dbAdapterForName', () {
    test('an absent define selects the ffi default', () {
      expect(dbAdapterForName('').name, 'ffi');
    });

    test('the two known names select their own adapter', () {
      expect(dbAdapterForName('ffi'), isA<Sqlite3Adapter>());
      expect(dbAdapterForName('sqflite'), isA<SqfliteAdapter>());
    });

    test('an unrecognized name throws instead of defaulting', () {
      // The pre-rename `sqlite3` spelling is exactly the typo this guards.
      expect(() => dbAdapterForName('sqlite3'), throwsArgumentError);
      expect(() => dbAdapterForName('FFI'), throwsArgumentError);
    });
  });

  group('Sqlite3Adapter storage config (PROTOCOL §9.7 parity)', () {
    test('open applies journal_mode=WAL and foreign_keys=ON', () async {
      final path = '${Directory.systemTemp.path}/'
          'flutter_bench_storage_config_test.db';
      final file = File(path);
      if (await file.exists()) await file.delete();
      final adapter = Sqlite3Adapter();
      await adapter.open(path);
      try {
        final journal = await adapter.query('PRAGMA journal_mode');
        expect(journal.first.values.first, 'wal');
        final fk = await adapter.query('PRAGMA foreign_keys');
        expect(fk.first.values.first, 1);
      } finally {
        await adapter.close();
        for (final suffix in ['', '-wal', '-shm']) {
          final f = File('$path$suffix');
          if (await f.exists()) await f.delete();
        }
      }
    });
  });
}
