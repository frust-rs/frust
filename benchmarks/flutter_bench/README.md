# flutter_bench

The **Flutter side** of the Frust vs Flutter paired benchmark suite
(Phase 9.E). Its Frust counterpart is `benchmarks/frust_bench`; both implement
the same eight scenarios (S1–S8) behind the same contract, and one shared
harness (`benchmarks/harness`) drives both and computes identical statistics
from the raw traces. The methodology is published in `benchmarks/PROTOCOL.md`.

This app's source is deliberately idiomatic Flutter (const constructors,
`ListView.builder` virtualization, `RepaintBoundary` where it helps,
`Isolate.run` for heavy work, the `shared_preferences` package) — the
published-source fairness promise means it should read like a good-faith
Flutter implementation.

## Scenarios

| id | scenario | file |
|----|----------|------|
| s1 | Animation storm (60 gradient bubbles + text, per-frame physics) | `lib/scenarios/s1_animation.dart` |
| s2 | Long-list scroll (10k rows, scripted scroll timeline) | `lib/scenarios/s2_list.dart` |
| s3 | Table ops (js-framework-benchmark subset, per-op markers) | `lib/scenarios/s3_table.dart` |
| s4 | Heavy-work responsiveness (~50MB JSON parse via `Isolate.run` under animation) | `lib/scenarios/s4_heavy.dart` |
| s5 | Image pipeline (async decode of deterministic bytes under scroll) | `lib/scenarios/s5_image.dart` |
| s6 | Text shaping stress (multilingual corpus under width animation) | `lib/scenarios/s6_text.dart` |
| s7 | Cold start + 60s idle | `lib/scenarios/s7_cold_start.dart` |
| s8 | Plugin-call overhead (`shared_preferences` write/read matrix) | `lib/scenarios/s8_prefs.dart` |

## Running a scenario

Select via a `--dart-define` or a deep link (`flutterbench://<id>`):

```bash
flutter run --profile --dart-define=SCENARIO=s1
# or, on a running install:
adb shell am start -a android.intent.action.VIEW -d "flutterbench://s3"
```

With neither, the app shows a scenario picker.

Extra defines:

- `--dart-define=CAPTURE=0` — silence the raw trace (clean desktop smoke).
- `--dart-define=S8_BURST=1` — run S8 with an animation concurrently
  (the burst-during-animation UI-thread-impact variant).

## Trace output

Each frame emits one parseable line to the platform log (logcat), analogous to
the frust side's `frust-perf raw` line:

```
flutter-perf raw n=<frameNumber> build_us=<build> raster_us=<raster> total_us=<total>
```

Scenario windows are bracketed by byte-identical markers
(`bench-scenario-start <name>` / `bench-scenario-end <name>`); S3/S4/S8 emit
sub-op markers (`s3-create1k`, `s4-parse`, `s8-write`, …). See
`lib/bench/perf.dart` for the exact contract and field mapping.

## Fairness gates

- **Android high refresh**: opted into explicitly at startup via
  `flutter_displaymode` (Flutter does not default to >60Hz).
- **iOS ProMotion**: `CADisableMinimumFrameDurationOnPhone` in
  `ios/Runner/Info.plist`.
- **Identical inputs**: all datasets are seeded through `lib/bench/rng.dart`, a
  byte-for-byte port of the frust `SplitMix64`, so both apps generate the same
  bubble layouts, image bytes, and JSON payloads.

## Verify

```bash
flutter analyze
flutter test
flutter build apk --profile
```
