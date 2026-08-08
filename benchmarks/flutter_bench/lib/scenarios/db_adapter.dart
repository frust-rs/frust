/// The DB-adapter seam the `d1`/`d2` scenarios (PROTOCOL §9) drive — one
/// small interface, two idiomatic implementations, so the scenario code
/// itself never branches on which SQLite story it is exercising (PROTOCOL
/// §9.7's fairness stance: "same SQL, same transaction boundaries").
///
/// - [Sqlite3Adapter] — `package:sqlite3`, in-process FFI, synchronous
///   underneath (wrapped in `async` only to satisfy the shared interface).
///   This is PROTOCOL's **engine-parity column**.
/// - [SqfliteAdapter] — `sqflite`, an async platform-channel round trip to
///   the OS-provided SQLite. This is PROTOCOL's **ecosystem-typical column**.
///
/// **Never add `sqlite3_flutter_libs`** (see `pubspec.yaml`'s comment) — its
/// presence would silently change which SQLite build either adapter
/// resolves, an invisible fairness bug per PROTOCOL §9.7.
///
/// Adapter selection mirrors how `SCENARIO` reaches the app (PROTOCOL's iOS
/// mechanism, `registry.dart`'s `resolveScenarioId`): a compile-time
/// `--dart-define=DB_ADAPTER=ffi|sqflite`, defaulting to `ffi`.
library;

import 'dart:io';

import 'package:sqflite/sqflite.dart' as sqflite;
import 'package:sqlite3/sqlite3.dart' as sqlite3lib;

/// The on-device path for a scenario's DB file — a fresh file per run (any
/// pre-existing file at this path is deleted first), named by scenario id and
/// adapter so d1/d2 × ffi/sqflite runs never collide. [Directory.systemTemp]
/// mirrors `bench/perf.dart`'s `benchTracePath` convention — already proven
/// to resolve to a writable per-app directory on both Android and iOS.
Future<String> dScenarioDbPath(String scenarioId, String adapterName) async {
  final path =
      '${Directory.systemTemp.path}/flutter_bench_${scenarioId}_$adapterName.db';
  final file = File(path);
  if (await file.exists()) {
    await file.delete();
  }
  return path;
}

/// One open DB connection (or an in-flight transaction on it), speaking the
/// exact same four-method surface regardless of which SQLite story is behind
/// it. `execute`/`query` take positional `?` parameters — the same SQL string
/// and the same parameter list are issued by scenario code on both adapters,
/// which is what makes the d1/d2 fairness comparison valid.
abstract class DbAdapter {
  /// Adapter name for logging (`ffi` / `sqflite`) — carried on the scenario's
  /// one-shot version-info line (PROTOCOL §9.7's per-run version-recording
  /// requirement).
  String get name;

  /// Open (creating if absent) the database file at [path].
  Future<void> open(String path);

  /// Execute a non-query statement (DDL, INSERT/UPDATE/DELETE) with
  /// positional `?` parameters.
  Future<void> execute(String sql, [List<Object?> params]);

  /// Execute a `SELECT` and return its rows as column-name→value maps.
  Future<List<Map<String, Object?>>> query(String sql, [List<Object?> params]);

  /// Run [fn] inside one transaction. Every `execute`/`query` call issued on
  /// the [DbAdapter] handed to [fn] participates in that one transaction —
  /// [fn] MUST use that handle, not the outer adapter, mirroring both
  /// underlying APIs' single-active-transaction model (`sqlite3`'s manual
  /// `BEGIN`/`COMMIT`, `sqflite`'s `Transaction` handle). A thrown exception
  /// inside [fn] rolls the transaction back and rethrows.
  Future<void> transaction(Future<void> Function(DbAdapter txn) fn);

  /// The concrete SQLite engine version actually linked/opened — recorded
  /// once at scenario start (PROTOCOL §9.7: "Each side's exact SQLite version
  /// actually linked is recorded per run ... not assumed from a package's
  /// declared minimum").
  Future<String> engineVersion();

  /// Close the underlying connection.
  Future<void> close();
}

/// Selects the adapter named by the `DB_ADAPTER` compile-time define
/// (`ffi` | `sqflite`), defaulting to `ffi` — the same
/// `String.fromEnvironment` mechanism `registry.dart` uses for `SCENARIO`.
DbAdapter selectDbAdapter() {
  const adapter = String.fromEnvironment('DB_ADAPTER', defaultValue: 'ffi');
  switch (adapter) {
    case 'sqflite':
      return SqfliteAdapter();
    case 'ffi':
    default:
      return Sqlite3Adapter();
  }
}

/// **Engine-parity column** (PROTOCOL §9.7) — `package:sqlite3`, an
/// in-process FFI binding whose synchronous API is wrapped in `async` here
/// only to satisfy [DbAdapter]'s shared surface; no platform channel, no I/O
/// isolate hop.
class Sqlite3Adapter implements DbAdapter {
  sqlite3lib.Database? _db;

  @override
  String get name => 'ffi';

  @override
  Future<void> open(String path) async {
    _db = sqlite3lib.sqlite3.open(path);
  }

  sqlite3lib.Database get _requireDb {
    final db = _db;
    if (db == null) {
      throw StateError('Sqlite3Adapter.open() was not called');
    }
    return db;
  }

  @override
  Future<void> execute(String sql, [List<Object?> params = const []]) async {
    _requireDb.execute(sql, params);
  }

  @override
  Future<List<Map<String, Object?>>> query(String sql,
      [List<Object?> params = const []]) async {
    final result = _requireDb.select(sql, params);
    return [
      for (final row in result) Map<String, Object?>.from(row),
    ];
  }

  @override
  Future<void> transaction(Future<void> Function(DbAdapter txn) fn) async {
    final db = _requireDb;
    db.execute('BEGIN');
    try {
      await fn(this);
      db.execute('COMMIT');
    } catch (_) {
      db.execute('ROLLBACK');
      rethrow;
    }
  }

  @override
  Future<String> engineVersion() async =>
      sqlite3lib.sqlite3.version.libVersion;

  @override
  Future<void> close() async {
    _db?.close();
    _db = null;
  }
}

/// **Ecosystem-typical column** (PROTOCOL §9.7) — `sqflite`, an async
/// `MethodChannel` round trip to the OS-provided SQLite (version is
/// device-variable, recorded via [engineVersion] rather than assumed).
class SqfliteAdapter implements DbAdapter {
  sqflite.Database? _db;

  @override
  String get name => 'sqflite';

  @override
  Future<void> open(String path) async {
    _db = await sqflite.openDatabase(path);
  }

  sqflite.Database get _requireDb {
    final db = _db;
    if (db == null) {
      throw StateError('SqfliteAdapter.open() was not called');
    }
    return db;
  }

  @override
  Future<void> execute(String sql, [List<Object?> params = const []]) =>
      _requireDb.execute(sql, params);

  @override
  Future<List<Map<String, Object?>>> query(String sql,
          [List<Object?> params = const []]) =>
      _requireDb.rawQuery(sql, params);

  @override
  Future<void> transaction(Future<void> Function(DbAdapter txn) fn) =>
      _requireDb.transaction((txn) => fn(_SqfliteTxnAdapter(txn)));

  @override
  Future<String> engineVersion() async {
    final rows = await _requireDb.rawQuery('SELECT sqlite_version() AS v');
    return rows.first['v'] as String;
  }

  @override
  Future<void> close() async {
    await _db?.close();
    _db = null;
  }
}

/// Wraps a `sqflite` [sqflite.Transaction] handle as a [DbAdapter] — the
/// object [SqfliteAdapter.transaction] passes to its callback, so scenario
/// code inside a transaction issues the identical `execute`/`query` calls it
/// would outside one. Nested transactions, `open`, `close`, and
/// `engineVersion` are not meaningful on a transaction handle and throw.
class _SqfliteTxnAdapter implements DbAdapter {
  _SqfliteTxnAdapter(this._txn);

  final sqflite.Transaction _txn;

  @override
  String get name => 'sqflite';

  @override
  Future<void> open(String path) =>
      throw UnsupportedError('already open (inside a transaction)');

  @override
  Future<void> execute(String sql, [List<Object?> params = const []]) =>
      _txn.execute(sql, params);

  @override
  Future<List<Map<String, Object?>>> query(String sql,
          [List<Object?> params = const []]) =>
      _txn.rawQuery(sql, params);

  @override
  Future<void> transaction(Future<void> Function(DbAdapter txn) fn) =>
      throw UnsupportedError('nested transactions are not supported');

  @override
  Future<String> engineVersion() =>
      throw UnsupportedError('not meaningful inside a transaction');

  @override
  Future<void> close() =>
      throw UnsupportedError('cannot close inside a transaction');
}
