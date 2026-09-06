# Frust vs Flutter Benchmark Suite

Two paired apps implementing eight identical scenarios (S1–S8), one
harness that drives both and computes identical statistics from their raw
frame series, published methodology, and a results table.

**Purpose, scoped narrowly.** `benchmarks/` exists ONLY for flutter-vs-frust
comparative benchmarking per `PROTOCOL.md` — the S1–S8 frame-class scenarios
plus the D1/D2 DB op-latency scenarios (§9), each implemented identically by
both apps and measured by the shared harness. Exploratory scenarios, capability
probes, or single-sided (frust-only) demos that don't fit that paired-comparison
contract belong in `examples/playground`, not here.

## What's here

| Path | What |
|---|---|
| `PROTOCOL.md` | The published methodology: device matrix, environmental controls, run counts, statistics, fairness gates, the exact raw-line formats both apps emit, and the S1–S8 scenario table. Read this first. |
| `RESULTS.md` | Per-device × per-scenario results tables for the current renderer pass, a methodology-deviations section per device, the release app-size table, and the one-off renderer-transition (vello → frust-engine) comparison. Strictly Frust-vs-Flutter — engine-plan gate evidence lives in git history, not here. |
| `frust_bench/` | The Frust bench app — a standalone Cargo package (own workspace, path-deps into `crates/*`), one binary implementing all eight scenarios behind a scenario driver, selected via launch arg/deep link. |
| `flutter_bench/` | The Flutter bench app — idiomatic Flutter implementing the same eight scenarios, source published for scrutiny (see `PROTOCOL.md` §6's fairness gates). |
| `harness/` | Shell/Python scripts: `matrix.sh` runs the whole S1–S8 matrix on one device unattended (device state, installs, interleaved `run.sh` blocks, sanity/retry, S7 extras, sanitized raw staging); `run.sh` drives one scenario N times on one app; `device_state.sh` is the Android fairness gate; `stats.py` is the ONE shared statistics script; `summarize.py` turns a raw tree into the `RESULTS.md` tables (+ JSON); `app_size.sh` measures release artifacts; `ab_matrix.sh` is the frust-only render-arm gate driver. |
| `raw/` | Committed, sanitized raw per-run series backing every number in `RESULTS.md`: `raw/<device>/{frust_profile,flutter}/<sN>/run-NN.log` (+ Android `run-NN.pss_*.txt` meminfo snapshots, `stats.txt`, S7 `coldstart.txt`/`cpuinfo.txt`). |

## Why this exists

Frust's Rust-advantage narrative (see the parent plan's TL;DR/Background)
makes specific, falsifiable claims about performance relative to Flutter —
this suite is how those claims get measured rather than asserted. Every
number published in `RESULTS.md` is reproducible by anyone with the same
device via the commands below; every methodology decision (run count,
warmup discard, statistics) is declared explicitly in `PROTOCOL.md` because
no industry-standard cross-framework UI benchmark protocol exists to defer
to.

## How to run

```bash
# Build the two profile artifacts (from each app's own directory)
(cd benchmarks/frust_bench && frust build apk --profile --define FRUST_TRACE_RAW=1)
(cd benchmarks/flutter_bench && flutter build apk --profile)

# Full S1-S8 matrix on one Android device, unattended (see matrix.sh's header)
./benchmarks/harness/matrix.sh --platform android --device <serial> --device-name <slug> \
    --out <dir> --frust-apk <apk> --flutter-apk <apk>

# One scenario, one app (what matrix.sh calls per block)
./benchmarks/harness/run.sh s1 --app frust --device <serial>

# Tables for RESULTS.md from a staged raw tree
python3 benchmarks/harness/summarize.py --frust <dir>/raw/<slug>/frust_profile --flutter <dir>/raw/<slug>/flutter
```

iOS uses `--platform ios` with a signed `frust build ios --profile` app and one
`flutter build ios --profile --dart-define=SCENARIO=<sN>` app per scenario (see
`run.sh`'s header). Each standalone app can also be run/built directly
from its own directory for iteration:

```bash
(cd benchmarks/frust_bench && cargo run -- --scenario s1)
(cd benchmarks/flutter_bench && flutter run --profile)
```

## Status

`RESULTS.md` reflects real device runs only — never placeholder or projected
numbers. The current pass (2026-09-05) measures the frust-owned `frust-engine`
renderer; the vello-era pass it replaced is in git history (PROTOCOL §2.6).
