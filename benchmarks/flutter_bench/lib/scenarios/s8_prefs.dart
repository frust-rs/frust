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
/// Per-op lines: `flutter-perf plugin op=<phase> type=<t> n=<i> us=<micros>`;
/// phase windows are also bracketed by `s8-write` / `s8-read-cached` /
/// `s8-read-crossing` markers for wall-time slicing.
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

    // --- writes (every setX crosses the channel) ---
    markScenarioStart('s8-write');
    for (final type in _PrefType.values) {
      for (var i = 0; i < _keysPerType; i++) {
        final key = 's8_${type.name}_$i';
        final us = await _timeMicros(() => _write(prefs, type, key, i));
        benchEmit('flutter-perf plugin op=write type=${type.name} n=$i us=$us');
      }
    }
    markScenarioEnd('s8-write');

    // --- cached reads (Dart-memory cache, no channel crossing) ---
    markScenarioStart('s8-read-cached');
    for (final type in _PrefType.values) {
      for (var i = 0; i < _keysPerType; i++) {
        final key = 's8_${type.name}_$i';
        final us = _timeMicrosSync(() => _read(prefs, type, key));
        benchEmit(
            'flutter-perf plugin op=read_cached type=${type.name} n=$i us=$us');
      }
    }
    markScenarioEnd('s8-read-cached');

    // --- channel-crossing reads (reload() is the only crossing read path) ---
    markScenarioStart('s8-read-crossing');
    for (var i = 0; i < _keysPerType; i++) {
      final us = await _timeMicros(prefs.reload);
      benchEmit('flutter-perf plugin op=read_crossing type=reload n=$i us=$us');
    }
    markScenarioEnd('s8-read-crossing');

    if (!mounted) return;
    setState(() => _status = 'S8 complete '
        '(${_burst ? 'burst-during-animation' : 'quiescent'})');
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

  Future<void> _write(
      SharedPreferences prefs, _PrefType type, String key, int i) {
    switch (type) {
      case _PrefType.boolT:
        return prefs.setBool(key, i.isEven);
      case _PrefType.intT:
        return prefs.setInt(key, i);
      case _PrefType.doubleT:
        return prefs.setDouble(key, i * 1.5);
      case _PrefType.stringT:
        return prefs.setString(key, 'value_$i');
      case _PrefType.stringListT:
        return prefs.setStringList(key, ['a$i', 'b$i', 'c$i']);
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
