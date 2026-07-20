/// Raw per-frame capture, scenario markers, and startup spans — the Flutter
/// side of the paired-benchmark trace contract (Phase 9.E).
///
/// The frust side (`crates/frust-shell-common/src/perf.rs`) emits, per frame,
/// one `frust-perf raw n=.. total_us=.. rebuild_us=.. layout_us=.. paint_us=..
/// encode_present_us=.. skipped=0|1` line plus `bench-scenario-start/end NAME`
/// markers, all via `log::info!` (logcat on device). This module
/// emits the analogous Flutter series so ONE shared stats script can slice
/// both by identical markers and compute identical percentiles.
///
/// ## Field mapping (documented for the harness)
///
/// Flutter's [FrameTiming] exposes only two engine-phase durations, so the
/// raw line is intentionally narrower than the frust side's four passes:
///
/// ```text
/// flutter-perf raw n=<frameNumber> build_us=<build> raster_us=<raster> total_us=<total>
/// ```
///
/// - `build_us`  — [FrameTiming.buildDuration], the UI-thread build+layout+paint
///   phase. Analogous to the frust side's `rebuild_us + layout_us + paint_us`.
/// - `raster_us` — [FrameTiming.rasterDuration], the raster-thread GPU encode
///   + present. Analogous to the frust side's `encode_present_us`.
/// - `total_us`  — [FrameTiming.totalSpan], vsync-start to raster-finish, the
///   frame's wall time. Directly comparable to the frust side's `total_us`.
///
/// There is no `skipped` field: Flutter's [SchedulerBinding.addTimingsCallback]
/// only reports frames the engine actually rendered (it has no analog of the
/// frust mobile dirty-gate's skipped frame), so every reported frame is a real
/// one. The harness treats the two `total_us` series as the comparable column.
///
/// ## Scenario markers
///
/// Byte-identical strings to the frust side: `bench-scenario-start <name>` and
/// `bench-scenario-end <name>`. The top-level window uses the scenario id
/// (`s1`..`s8`); scenarios that time sub-operations emit their own sub-markers
/// with the same API (e.g. `s3-create1k`, `s4-parse`) — matching the frust
/// scenario tasks' naming.
///
/// ## Output sink
///
/// Every line goes through [benchEmit] → `print`, which reaches the platform
/// log (logcat / `flutter logs`) in profile mode on device without a host test
/// runner — the same place `log::info!` lands on the frust side.
library;

import 'dart:async';
import 'dart:io';

import 'package:flutter/scheduler.dart';

/// Uptime since `main()` entry — started by `main` before `runApp`, read by
/// S7 to report the in-app portion of cold start (`main` → first frame).
final Stopwatch benchUptime = Stopwatch();

/// Log-line prefix for the raw per-frame export — parallels the frust side's
/// `frust-perf raw` prefix.
const String rawFramePrefix = 'flutter-perf raw';

/// Log-line prefix for the one-shot startup span line (S7).
const String startupPrefix = 'flutter-perf startup';

/// The on-device trace file path (iOS only), under the app's temporary
/// directory (`NSTemporaryDirectory()` → the appDataContainer's `tmp/`), the
/// harness pulls after each run with `xcrun devicectl device copy from
/// --domain-type appDataContainer`.
final String benchTracePath =
    '${Directory.systemTemp.path}/flutter_bench_trace.log';

IOSink? _traceSink;
// Retained so the periodic flush Timer is not garbage-collected mid-capture.
// ignore: unused_element
Timer? _flushTimer;

void _ensureSink() {
  if (_traceSink != null) return;
  // FileMode.write truncates any prior run's file so each capture is clean.
  _traceSink = File(benchTracePath).openWrite(mode: FileMode.write);
  // Flush on a cadence so a mid-run `devicectl process terminate` (SIGTERM,
  // which does not run Dart finalizers) loses at most ~1s of buffered frames.
  _flushTimer = Timer.periodic(const Duration(seconds: 1), (_) {
    _traceSink?.flush();
  });
}

/// Emit one benchmark trace line to the platform's capture sink.
///
/// On **Android/desktop** this is `print` (not `debugPrint`, which throttles
/// high-volume output and can drop per-frame lines): `print` reaches logcat /
/// stdout in profile mode, which is what the Android harness scrapes.
///
/// On a physical **iOS** device neither `print` (routes to os_log, which the
/// modern-iOS syslog relay / `devicectl --console` does not surface) nor
/// `stdout.writeln` from the engine's post-frame `addTimingsCallback`
/// reliably reaches the host, so the same line is additionally appended to a
/// file in the app's `tmp/` container ([benchTracePath]) that the harness
/// pulls afterward. The file write lives off the measured path (the engine
/// captures each [FrameTiming] independent of when we serialize it), so the
/// sink choice does not affect any reported percentile. Callers pass a single
/// already-formatted line.
void benchEmit(String line) {
  print(line); // ignore: avoid_print
  if (Platform.isIOS) {
    _ensureSink();
    _traceSink!.writeln(line);
  }
}

/// Whether frame capture and markers are active. On by default (this is a
/// benchmark app); a `--dart-define=CAPTURE=0` run silences the trace for a
/// clean desktop smoke.
bool get captureEnabled =>
    const String.fromEnvironment('CAPTURE', defaultValue: '1') != '0';

/// Registers the per-frame timings callback exactly once. Each reported
/// [FrameTiming] becomes one [rawFramePrefix] line. Safe to call before
/// `runApp`; a no-op when [captureEnabled] is false.
void installFrameCapture() {
  if (!captureEnabled) return;
  SchedulerBinding.instance.addTimingsCallback(_onTimings);
}

void _onTimings(List<FrameTiming> timings) {
  for (final t in timings) {
    benchEmit(formatRawFrameLine(
      n: t.frameNumber,
      buildUs: t.buildDuration.inMicroseconds,
      rasterUs: t.rasterDuration.inMicroseconds,
      totalUs: t.totalSpan.inMicroseconds,
    ));
  }
}

/// Formats one raw per-frame line. Pure and directly unit-testable — the shape
/// must survive a `key=value`-splitting harness (see this file's test).
String formatRawFrameLine({
  required int n,
  required int buildUs,
  required int rasterUs,
  required int totalUs,
}) =>
    '$rawFramePrefix n=$n build_us=$buildUs raster_us=$rasterUs '
    'total_us=$totalUs';

/// Which edge of a scenario window a marker stamps.
enum _MarkerEdge {
  start('bench-scenario-start'),
  end('bench-scenario-end');

  const _MarkerEdge(this.prefix);
  final String prefix;
}

/// Formats a scenario marker line — separated from [markScenarioStart]/
/// [markScenarioEnd] for the same directly-testable reason the frust side
/// separates `format_scenario_marker`.
String formatScenarioMarker(String prefix, String name) => '$prefix $name';

/// Stamps `bench-scenario-start <name>`. No-op when capture is off.
void markScenarioStart(String name) {
  if (!captureEnabled) return;
  benchEmit(formatScenarioMarker(_MarkerEdge.start.prefix, name));
}

/// Stamps `bench-scenario-end <name>`. No-op when capture is off.
void markScenarioEnd(String name) {
  if (!captureEnabled) return;
  benchEmit(formatScenarioMarker(_MarkerEdge.end.prefix, name));
}

/// Emit the one-shot startup span line (S7). `firstFrameMs` is the in-app
/// portion: `main()` entry to first rasterized frame. True cold start (process
/// spawn → `main`) is only measurable externally via `am start -W`, which the
/// harness records separately — this line is the framework's own first-frame
/// span, per PLAN S7 ("report Flutter's own first-frame timeline events").
void emitStartupSpan({required int firstFrameMs}) {
  if (!captureEnabled) return;
  benchEmit('$startupPrefix first_frame_ms=$firstFrameMs');
}
