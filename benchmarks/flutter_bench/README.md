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
| d1 | DB writes (batched-transaction insert + autocommit single inserts) | `lib/scenarios/d1_db_write.dart` |
| d2 | DB reads (point selects + range scan) | `lib/scenarios/d2_db_read.dart` |

`d1`/`d2` are op-latency scenarios (PROTOCOL §9), a second scenario-id
namespace parallel to `s1..s8` — see `benchmarks/PROTOCOL.md` §9. Both run
against whichever `DbAdapter` `db_adapter.dart` selects: `package:sqlite3`
(in-process FFI, the engine-parity column) by default, or `sqflite`
(platform channel, the ecosystem-typical column) via
`--dart-define=DB_ADAPTER=sqflite`.

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
- `--dart-define=DB_ADAPTER=ffi|sqflite` — select d1/d2's DB adapter
  (default `ffi`, `package:sqlite3`).

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

`d1`/`d2` are op-latency, not per-frame — each op emits PROTOCOL §7's
canonical per-op line instead of a `-raw` frame line:

```
flutter-perf op scenario=<d1|d2> op=<name> n=<u64> us=<u64> err=<0|1> [<key>=<value> ...]
```

See `lib/scenarios/d1_db_write.dart` (`formatDbOpLine`) for the formatter and
`benchmarks/PROTOCOL.md` §9 for the full workload spec.

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
