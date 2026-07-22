# Lab 8 — The Measurement Lab

**Concept:** Every earlier chapter used `FRUST_TRACE` casually. This chapter
is the full instrument panel: raw per-frame export, startup spans, scenario
markers, and the paired Frust-vs-Flutter benchmark harness. The goal is a
habit: **never reason about rendering performance in this repo without a
trace line to point at.**

## The dials (with parse sites)

| Variable | Effect | Parsed at |
|---|---|---|
| `FRUST_TRACE` | Master switch: `FrameStats` summaries + `StartupSpans` | `crates/frust-shell-common/src/perf.rs` ≈75–111 |
| `FRUST_TRACE_RAW` | One parseable line **per frame** instead of summaries. **Requires `FRUST_TRACE` too** — alone it does nothing (two-dial contract, `perf.rs` ≈119–128) | `perf.rs` ≈114–135 |
| `FRUST_LOG` | Desktop stderr log level (the sink perf lines print through); `debug` un-suppresses vello noise | `crates/frust-shell-desktop/src/logger.rs` ≈103 |
| `FRUST_RENDER_TIER` | `gpu`/`cpu` tier override (lab 4.2) | `crates/frust-render/src/tier.rs` ≈187–216 |
| `FRUST_NO_FRAME_GATE` | Gate kill switch (lab 7.1) | `crates/frust-shell-common/src/frame_gate.rs` ≈63 |
| `FRUST_NO_RESAMPLE` | Resampler kill switch (lab 7.3) | `crates/frust-shell-common/src/resample.rs` ≈54 |

All the shell-common ones parse compile-time (`option_env!`) *or* runtime —
so `--define FRUST_TRACE=1` at build and `FRUST_TRACE=1` at launch both work
(`frust run --profile` auto-injects it).

## The output formats (so you can parse, not squint)

**Startup**, once, at first presented frame (`perf.rs` ≈687–696) —
milestones are ms-deltas from process begin, the `SPAN_*` consts at
`perf.rs` ≈559–589: `native_lib_load`, `init_entry`, `adapter_ready`,
`device_ready`, `renderer_ready`, `pipeline_cache_restored`,
`first_rebuild_done`, `first_encode_done`, `first_frame_presented`.

**Summary** (default), every ~2s of frames (`perf.rs` ≈403–427): p50/p95/p99
totals, per-pass p95s (`rebuild/layout/paint/encode/present`), 60Hz/120Hz
budget-overrun counts, `skipped`.

**Raw v2** (`FRUST_TRACE=1 FRUST_TRACE_RAW=1`), one line per frame,
formatted at `perf.rs` ≈474–489:

```
frust-perf raw n=<u64> total_us=.. rebuild_us=.. layout_us=.. paint_us=.. encode_us=.. present_us=.. skipped=<0|1>
```

plus `bench-scenario-start/end <name>` markers (`perf.rs` ≈507–535) and
free-form `bench_emit` lines (≈549) for per-op scenario measurements.

## The harness (`benchmarks/`)

Paired Frust/Flutter apps implementing the same eight scenarios
(`benchmarks/frust_bench/src/scenarios/mod.rs` ≈82–91; described in
`benchmarks/PROTOCOL.md` ≈256–271):

S1 animation storm (bubblebench workload) · S2 long-list scroll (10k rows) ·
S3 table ops · S4 heavy-work responsiveness (50MB JSON parse) · S5 image
pipeline · S6 text-shaping stress · S7 cold start + idle · S8 plugin-call
overhead.

```bash
./benchmarks/harness/run.sh <scenario> --app frust|flutter --device <serial> [--runs N]
```

Scenarios select via deep link (`frustbench://<id>`) or `FRUST_BENCH_SCENARIO`.
`benchmarks/PROTOCOL.md` is the methodology (fairness gates, run counts,
stats); `RESULTS.md` holds only *actual* device runs. macOS caveat: BSD
`mktemp` breaks `run.sh` — `brew install coreutils` and alias to `gmktemp`
(`docs/DEVELOPMENT.md`).

Also in the drawer: `scripts/size-report.sh` (release `.so`/APK size +
cargo-bloat) and `scripts/devloop-measure.sh` (incremental-build wall time,
defaults to bubblebench).

## Experiments

### 8.1 — Your first raw capture + histogram

```bash
cd examples/bubblebench
FRUST_TRACE=1 FRUST_TRACE_RAW=1 cargo run 2>&1 | tee /tmp/bb.raw
```

Interact for ~30s, quit, then slice it — the fields are `key=value`, so
even a one-liner works:

```bash
awk '/frust-perf raw/ {for(i=1;i<=NF;i++) if($i ~ /^paint_us=/){sub("paint_us=","",$i); print $i}}' /tmp/bb.raw | sort -n | awk '{a[NR]=$1} END {print "p50",a[int(NR*.5)], "p95",a[int(NR*.95)], "p99",a[int(NR*.99)]}'
```

(The harness's `stats.py` under `benchmarks/harness/` does this properly —
read it once.) Now re-run every experiment you eyeballed earlier with real
percentiles: gradient→solid (lab 2.1), bubble count sweep, `cpu` tier
(lab 4.2 — compare `encode_us` distributions GPU vs CPU).

### 8.2 — Decompose a cold start

`FRUST_TRACE=1 (cd examples/huddle && cargo run)` and read the one
`frust-perf startup` line. Which dominates on your machine —
`device_ready` (wgpu adapter+device) or `first_encode_done` (first vello
pipeline compile)? Match each milestone to the code that stamps it
(desktop: `app_handler.rs` ≈943/1042/1101; the rest during render-context
init, chapter 4). This is the map you'll need the day cold start regresses.

### 8.3 — Run one real benchmark scenario

With an Android device attached:

```bash
./benchmarks/harness/run.sh s1 --app frust --device <serial> --runs 3
```

Then read your run against the recorded OnePlus 9 matrix in
`benchmarks/RESULTS.md` — and read `PROTOCOL.md`'s fairness gates to see
how much discipline separates "a number" from "a result." If you change
anything render-path-adjacent later, this is the regression check.

### 8.4 — Mark your own scenario

`mark_scenario_start/end` (`perf.rs` ≈507–535) is public shell-common API.
Wrap something you care about in bubblebench (e.g. the first 10s after
launch vs. after a Reset) and slice your raw log by the marker lines. You
now have the same tooling the S1–S8 suite uses, on your own workload.

## Where to go from here

- The per-pass numbers name their own next chapter: high `paint_us` → your
  widgets (ch. 2); high `encode_us` → command volume / vello pipeline
  (ch. 4–5); high `rebuild_us` → view diffing and signal graph (ch. 3);
  `skipped` anomalies → the gate (ch. 7).
- When a question outgrows this repo's instruments (per-shader GPU timings),
  lab 5.4's Metal capture / RenderDoc is the escalation path — and by then
  the external roadmap's profiling section will read as a tool manual, not
  theory.
