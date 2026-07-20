# Frust vs Flutter Benchmark Suite

Two paired apps implementing eight identical scenarios (S1–S8), one
harness that drives both and computes identical statistics from their raw
frame series, published methodology, and a results table. This is the
deliverable of Phase 9.E in
`workflow/plans/features/frust-phase-9-rust-advantage/PLAN.md`.

## What's here

| Path | What |
|---|---|
| `PROTOCOL.md` | The published methodology: device matrix, environmental controls, run counts, statistics, fairness gates, the exact raw-line formats both apps emit, and the S1–S8 scenario table. Read this first. |
| `RESULTS.md` | Per-device × per-scenario results tables (empty template until runs are recorded), plus a methodology-deviations section per device. |
| `frust_bench/` | The Frust bench app — a standalone Cargo package (own workspace, path-deps into `crates/*`), one binary implementing all eight scenarios behind a scenario driver, selected via launch arg/deep link. |
| `flutter_bench/` | The Flutter bench app — idiomatic Flutter implementing the same eight scenarios, source published for scrutiny (see `PROTOCOL.md` §6's fairness gates). |
| `harness/` | Shell/Python scripts: install both apps, set device state (brightness/airplane mode/thermal cooldown), run a scenario N times, pull logs, slice by scenario markers, compute percentile statistics from ONE shared script, and emit the `RESULTS.md` table + raw CSVs. |
| `raw/` | Committed raw per-run series (CSV) backing every number in `RESULTS.md` — created once `07-run-matrix` runs the suite; not present until then. |

## Why this exists

Frust's Rust-advantage narrative (see the parent plan's TL;DR/Background)
makes specific, falsifiable claims about performance relative to Flutter —
this suite is how those claims get measured rather than asserted. Every
number published in `RESULTS.md` is reproducible by anyone with the same
device via the commands below; every methodology decision (run count,
warmup discard, statistics) is declared explicitly in `PROTOCOL.md` because
no industry-standard cross-framework UI benchmark protocol exists to defer
to (see `research/RESEARCH.md` §7/§8's refuted-claims ledger in the parent
plan).

## How to run

```bash
# Run one scenario on a connected device, both apps, full matrix
./benchmarks/harness/run.sh s1 --device <serial>

# Run the full S1-S8 matrix
./benchmarks/harness/run.sh all --device <serial>
```

See `harness/README.md` (once `06-harness` lands) for install prerequisites
and device-state setup. Each standalone app can also be run/built directly
from its own directory for iteration:

```bash
(cd benchmarks/frust_bench && cargo run -- --scenario s1)
(cd benchmarks/flutter_bench && flutter run --profile)
```

## Status

Both apps and the harness are being built out across Phase 9.E's tasks
(`workflow/plans/features/frust-phase-9-rust-advantage/9e/TASKS.md`).
`RESULTS.md` reflects real device runs only — it starts empty and fills in
as the matrix is actually executed, never with placeholder or projected
numbers.
