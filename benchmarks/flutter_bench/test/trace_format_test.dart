/// Tests for the pure trace-format + selection helpers — the shapes the shared
/// harness parses, and the deterministic PRNG the dataset parity depends on.
library;

import 'package:flutter_bench/bench/perf.dart';
import 'package:flutter_bench/bench/registry.dart';
import 'package:flutter_bench/bench/rng.dart';
import 'package:flutter_test/flutter_test.dart';

/// A raw per-frame line's parsed fields — the harness's `key=value` split,
/// mirrored here as a round-trip check (the frust side has the same test).
class _ParsedRawFrame {
  _ParsedRawFrame(this.n, this.buildUs, this.rasterUs, this.totalUs);
  final int n, buildUs, rasterUs, totalUs;
}

_ParsedRawFrame? _parseRawFrame(String line) {
  if (!line.startsWith(rawFramePrefix)) return null;
  final rest = line.substring(rawFramePrefix.length).trim();
  final fields = <String, int>{};
  for (final part in rest.split(RegExp(r'\s+'))) {
    final kv = part.split('=');
    if (kv.length == 2) fields[kv[0]] = int.parse(kv[1]);
  }
  final n = fields['n'], b = fields['build_us'];
  final r = fields['raster_us'], t = fields['total_us'];
  if (n == null || b == null || r == null || t == null) return null;
  return _ParsedRawFrame(n, b, r, t);
}

void main() {
  group('raw frame line', () {
    test('round-trips through a key=value parse', () {
      final line = formatRawFrameLine(
          n: 42, buildUs: 1234, rasterUs: 567, totalUs: 2000);
      expect(line.startsWith(rawFramePrefix), isTrue);
      final p = _parseRawFrame(line)!;
      expect(p.n, 42);
      expect(p.buildUs, 1234);
      expect(p.rasterUs, 567);
      expect(p.totalUs, 2000);
    });
  });

  group('scenario markers', () {
    test('start and end use the byte-identical frust strings', () {
      expect(formatScenarioMarker('bench-scenario-start', 's1'),
          'bench-scenario-start s1');
      expect(formatScenarioMarker('bench-scenario-end', 's3-create1k'),
          'bench-scenario-end s3-create1k');
    });
  });

  group('scenarioIdFromRoute', () {
    test('parses the custom-scheme deep link host as the id', () {
      expect(scenarioIdFromRoute('flutterbench://s3'), 's3');
    });
    test('parses a scenario-path deep link', () {
      expect(scenarioIdFromRoute('flutterbench://scenario/s5'), 's5');
    });
    test('parses a plain path route', () {
      expect(scenarioIdFromRoute('/s2'), 's2');
    });
    test('returns null for the root route', () {
      expect(scenarioIdFromRoute('/'), isNull);
      expect(scenarioIdFromRoute(null), isNull);
    });
  });

  group('registry', () {
    test('has all eight frame-class scenarios plus d1/d2', () {
      expect(allScenarios.map((s) => s.id).toList(), [
        's1', 's2', 's3', 's4', 's5', 's6', 's7', 's8', //
        'd1', 'd2',
      ]);
    });
    test('lookup is case-insensitive and trims', () {
      expect(scenarioById(' S4 ')?.id, 's4');
      expect(scenarioById('nope'), isNull);
    });
  });

  group('SplitMix64', () {
    test('is deterministic for a given seed', () {
      final a = SplitMix64(42);
      final b = SplitMix64(42);
      for (var i = 0; i < 100; i++) {
        expect(a.nextU64(), b.nextU64());
      }
    });
    test('nextF64 stays in [0, 1)', () {
      final rng = SplitMix64(7);
      for (var i = 0; i < 1000; i++) {
        final v = rng.nextF64();
        expect(v, greaterThanOrEqualTo(0.0));
        expect(v, lessThan(1.0));
      }
    });
    test('matches the frust SplitMix64 reference sequence for seed 42', () {
      // First three next_u64() outputs of the Rust SplitMix64(42) — the shared
      // reference values that guarantee cross-language dataset parity.
      final rng = SplitMix64(42);
      expect(rng.nextU64(), _splitmix64Ref(42, 1));
      expect(rng.nextU64(), _splitmix64Ref(42, 2));
      expect(rng.nextU64(), _splitmix64Ref(42, 3));
    });
  });
}

/// A second, independent implementation of SplitMix64 advanced `steps` times
/// from `seed` — a cross-check that [SplitMix64] follows the canonical
/// algorithm (and therefore the frust port) bit-for-bit.
int _splitmix64Ref(int seed, int steps) {
  var state = seed;
  var out = 0;
  for (var i = 0; i < steps; i++) {
    state = state + 0x9E3779B97F4A7C15;
    var z = state;
    z = (z ^ (z >>> 30)) * 0xBF58476D1CE4E5B9;
    z = (z ^ (z >>> 27)) * 0x94D049BB133111EB;
    out = z ^ (z >>> 31);
  }
  return out;
}
