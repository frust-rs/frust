/// S8 — plugin-call overhead. `shared_preferences` (Flutter's idiomatic prefs
/// package) write + read loops across all five value types, per-op latency +
/// total wall time, with cached and channel-crossing reads reported separately
/// per PROTOCOL's S8 rules. The head-to-head for direct in-process FFI
/// (`frust-shared-preferences`) vs MethodChannel round-trips.
///
/// **S8 fairness (PROTOCOL)**: `shared_preferences` caches all values in Dart
/// memory after `getInstance()`, so a naive `getX` never crosses the channel.
/// We therefore separate:
///   - `write`         — every `setX` crosses the channel (the honest write cost),
///   - `read_cached`   — `getX` served from the Dart cache (cache quality, not
///                       boundary cost),
///   - `read_crossing` — `reload()` (the package's ONLY channel-crossing read
///                       path; it reloads the whole store) — reported as the
///                       crossing-read cost.
/// The claim under test is boundary cost, not cache quality — both are reported.
///
/// A `--dart-define=S8_BURST=1` run drives an S1-style animation concurrently
/// to measure UI-thread impact (PLAN's burst-during-animation variant).
///
/// Per-op lines: ```flutter-perf plugin op=<phase> type=<t> n=<i> us=<micros> err=0|1```
/// phase windows are also bracketed by `s8-write` / `s8-read-cached` /
/// `s8-read-crossing` markers for wall-time slicing.
///
/// **Error accounting** (mirroring the frust side's `s8_prefs.rs`): a write
/// whose `Future` throws increments `write_errors` and marks that key
/// write-failed so a read never double-counts the same root cause. Every
/// cached read verifies its value against the deterministic value `_write`
/// should have stored, counting an unexpectedly-absent key
/// (`read_unexpected_none`) or a present-but-wrong value
/// (`read_value_mismatch`) — skipped for a key whose write already failed.
/// `reload()` (the crossing-read phase) has no per-key value to verify, so a
/// thrown exception there counts toward `read_crossing_errors` instead (a
/// field the frust side has no analog of, since it has no separate
/// crossing-read phase). A nonzero total across any of these four counters
/// emits one parseable `s8-errors` marker line, matching the frust side's
/// field names where a concept is shared.
library;

import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../bench/perf.dart';

/// Keys per value type — unique keys so no cache can pre-serve a fresh read.
const int _keysPerType = 200;

/// Whether the burst-during-animation variant is active.
bool get _burst =>
    const String.fromEnvironment('S8_BURST', defaultValue: '0') != '0';

/// The five SharedPreferences value types, each with a writer and a reader.
enum _PrefType { boolT, intT, doubleT, stringT, stringListT }

class PluginOverheadView extends StatefulWidget {
  const PluginOverheadView({super.key});

  @override
  State<PluginOverheadView> createState() => _PluginOverheadViewState();
}

class _PluginOverheadViewState extends State<PluginOverheadView>
    with SingleTickerProviderStateMixin {
  AnimationController? _spin;
  String _status = 'starting…';

  @override
  void initState() {
    super.initState();
    if (_burst) {
      _spin = AnimationController(
        vsync: this,
        duration: const Duration(seconds: 2),
      )..repeat();
    }
    WidgetsBinding.instance.addPostFrameCallback((_) => _run());
  }

  Future<void> _run() async {
    final prefs = await SharedPreferences.getInstance();

    // Per-key write-success tracking so the read phase can skip
    // verification — never the timing — for a key whose write already
    // failed (no double-counting the same root cause).
    final writeOk = <String, bool>{};
    var writeErrors = 0;
    var readUnexpectedNone = 0;
    var readValueMismatch = 0;
    var readCrossingErrors = 0;

    // --- writes (every setX crosses the channel) ---
    markScenarioStart('s8-write');
    for (final type in _PrefType.values) {
      for (var i = 0; i < _keysPerType; i++) {
        final key = 's8_${type.name}_$i';
        var err = false;
        final us = await _timeMicros(() async {
          try {
            await _write(prefs, type, key, i);
          } catch (_) {
            err = true;
          }
        });
        if (err) writeErrors++;
        writeOk[key] = !err;
        benchEmit('flutter-perf plugin op=write type=${type.name} n=$i '
            'us=$us err=${err ? 1 : 0}');
      }
    }
    markScenarioEnd('s8-write');

    // --- cached reads (Dart-memory cache, no channel crossing), each
    // verified against the value _write should have stored ---
    markScenarioStart('s8-read-cached');
    for (final type in _PrefType.values) {
      for (var i = 0; i < _keysPerType; i++) {
        final key = 's8_${type.name}_$i';
        Object? got;
        final us = _timeMicrosSync(() {
          got = _read(prefs, type, key);
        });
        // verification deliberately excluded from the timed window — do not
        // reintroduce
        // Skip attributing a mismatch/none to this key if its write
        // already failed — that failure is already counted above.
        var err = false;
        if (writeOk[key] ?? true) {
          final expected = _expectedValue(type, i);
          if (got == null) {
            readUnexpectedNone++;
            err = true;
          } else if (!_valuesEqual(got, expected)) {
            readValueMismatch++;
            err = true;
          }
        }
        benchEmit('flutter-perf plugin op=read_cached type=${type.name} '
            'n=$i us=$us err=${err ? 1 : 0}');
      }
    }
    markScenarioEnd('s8-read-cached');

    // --- channel-crossing reads (reload() is the only crossing read path;
    // there is no per-key value to verify here, so a thrown exception is
    // the only error signal) ---
    markScenarioStart('s8-read-crossing');
    for (var i = 0; i < _keysPerType; i++) {
      var err = false;
      final us = await _timeMicros(() async {
        try {
          await prefs.reload();
        } catch (_) {
          err = true;
        }
      });
      if (err) readCrossingErrors++;
      benchEmit('flutter-perf plugin op=read_crossing type=reload n=$i '
          'us=$us err=${err ? 1 : 0}');
    }
    markScenarioEnd('s8-read-crossing');

    final totalErrors = writeErrors +
        readUnexpectedNone +
        readValueMismatch +
        readCrossingErrors;
    if (totalErrors > 0) {
      benchEmit('flutter-perf plugin s8-errors write_errors=$writeErrors '
          'read_unexpected_none=$readUnexpectedNone '
          'read_value_mismatch=$readValueMismatch '
          'read_crossing_errors=$readCrossingErrors');
    }

    if (!mounted) return;
    setState(() => _status = 'S8 complete '
        '(${_burst ? 'burst-during-animation' : 'quiescent'})'
        '${totalErrors > 0 ? ' — $totalErrors error(s): '
            'write=$writeErrors unexpected_none=$readUnexpectedNone '
            'mismatch=$readValueMismatch crossing=$readCrossingErrors' : ''}');
  }

  Future<int> _timeMicros(Future<void> Function() op) async {
    final sw = Stopwatch()..start();
    await op();
    sw.stop();
    return sw.elapsedMicroseconds;
  }

  int _timeMicrosSync(void Function() op) {
    final sw = Stopwatch()..start();
    op();
    sw.stop();
    return sw.elapsedMicroseconds;
  }

  /// The deterministic value `_write` stores (and cached-read verification
  /// expects back) for `(type, i)` — factored out so the write and
  /// read-verify paths can never drift from each other, mirroring the frust
  /// side's `expected_value`.
  Object _expectedValue(_PrefType type, int i) {
    switch (type) {
      case _PrefType.boolT:
        return i.isEven;
      case _PrefType.intT:
        return i;
      case _PrefType.doubleT:
        return i * 1.5;
      case _PrefType.stringT:
        return 'value_$i';
      case _PrefType.stringListT:
        return ['a$i', 'b$i', 'c$i'];
    }
  }

  /// Value equality for a verified read — `List` needs elementwise
  /// comparison since Dart's `==` on two distinct `List` instances is
  /// identity, not content, equality.
  bool _valuesEqual(Object? got, Object expected) {
    if (got is List && expected is List) {
      if (got.length != expected.length) return false;
      for (var i = 0; i < got.length; i++) {
        if (got[i] != expected[i]) return false;
      }
      return true;
    }
    return got == expected;
  }

  Future<void> _write(
      SharedPreferences prefs, _PrefType type, String key, int i) {
    final value = _expectedValue(type, i);
    switch (type) {
      case _PrefType.boolT:
        return prefs.setBool(key, value as bool);
      case _PrefType.intT:
        return prefs.setInt(key, value as int);
      case _PrefType.doubleT:
        return prefs.setDouble(key, value as double);
      case _PrefType.stringT:
        return prefs.setString(key, value as String);
      case _PrefType.stringListT:
        return prefs.setStringList(key, value as List<String>);
    }
  }

  Object? _read(SharedPreferences prefs, _PrefType type, String key) {
    switch (type) {
      case _PrefType.boolT:
        return prefs.getBool(key);
      case _PrefType.intT:
        return prefs.getInt(key);
      case _PrefType.doubleT:
        return prefs.getDouble(key);
      case _PrefType.stringT:
        return prefs.getString(key);
      case _PrefType.stringListT:
        return prefs.getStringList(key);
    }
  }

  @override
  void dispose() {
    _spin?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final spin = _spin;
    return Center(
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          if (spin != null)
            RotationTransition(
              turns: spin,
              child: const Icon(Icons.sync, size: 72),
            ),
          const SizedBox(height: 24),
          Text(_status),
        ],
      ),
    );
  }
}
