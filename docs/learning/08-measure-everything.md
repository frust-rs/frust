# Lab 8 — The Measurement Lab

**Concept:** Every earlier chapter used `FRUST_TRACE` casually. This chapter
is the full instrument panel: raw per-frame export, startup spans, scenario
markers, and the paired Frust-vs-Flutter benchmark harness. The goal is a
habit: **never reason about rendering performance in this repo without a
trace line to point at.**

## The dials (with parse sites)

| Variable | Effect | Parsed at |
|---|---|---|
| `FRUST_TRACE` | Master switch: `FrameStats` summaries + `StartupSpans` | `crates/frust-shell-common/src/perf.rs` ≈111–138 |
| `FRUST_TRACE_RAW` | One parseable line **per frame** instead of summaries. **Requires `FRUST_TRACE` too** — alone it does nothing (two-dial contract, `perf.rs` ≈177–181) | `perf.rs` ≈174–201 |
| `FRUST_LOG` | Desktop stderr log level (the sink perf lines print through); the logger applies no target-based filtering at all, only a level check — `debug` simply widens what already prints | `crates/frust-shell-desktop/src/logger.rs` ≈68 |
| `FRUST_NO_FRAME_GATE` | Gate kill switch (lab 7.1) | `crates/frust-shell-common/src/frame_gate.rs` ≈89 |
| `FRUST_NO_RESAMPLE` | Resampler kill switch (lab 7.3) | `crates/frust-shell-common/src/resample.rs` ≈54 |

All the shell-common ones parse compile-time (`option_env!`) *or* runtime —
so `--define FRUST_TRACE=1` at build and `FRUST_TRACE=1` at launch both work
(`frust run --profile` auto-injects it).

## The output formats (so you can parse, not squint)

**Startup**, once, at first presented frame (`StartupSpans::emit_log`, `perf.rs`
≈1396) — milestones are ms-deltas from process begin, the `SPAN_*` consts at
`perf.rs` ≈1270–1298: `native_lib_load`, `init_entry`, `adapter_ready`,
`device_ready`, `renderer_ready`, `pipeline_cache_restored`,
`first_rebuild_done`, `first_encode_done`, `first_frame_presented`.

**Summary** (default), every ~2s of frames (`perf.rs` ≈383 the
`FrameSummary` shape, `FrameStats::emit_log` ≈706): p50/p95/p99 totals, per-pass p95s
(`rebuild/layout/paint/encode/acquire/submit` — chapter 3's encode→acquire→submit
split), 60Hz/120Hz budget-overrun counts, `skipped`.

**Raw** (`FRUST_TRACE=1 FRUST_TRACE_RAW=1`), one line per frame, formatted at
`perf.rs`'s `format_raw_frame_line` ≈806 (v3, 2026-07-22, split the old v2
`present_us` into `acquire_us`+`submit_us`; see that function's doc comment for
the full v1→v2→v3 history):

```
frust-perf raw n=<u64> total_us=.. rebuild_us=.. layout_us=.. paint_us=.. encode_us=.. acquire_us=.. submit_us=.. skipped=<0|1> gpu_q=<0|1>
```

**Format v4** (2026-09-01) appends real per-pass GPU time additively once
`gpu_q=1`: `gpu_total_us`/`gpu_prepass_us`/`gpu_main_us`/`gpu_composite_us`/
`gpu_blit_us`, omitted entirely (not zeroed) when `gpu_q=0` — a v3 parser
reads a v4 line unchanged.

Plus `bench-scenario-start/end <name>` markers (`mark_scenario_start`/
`mark_scenario_end`, `perf.rs` ≈1228–1239) and free-form `bench_emit` lines
(≈1257) for per-op scenario measurements.

## The harness (`benchmarks/`)

Paired Frust/Flutter apps implementing the same eight *timed* scenarios
(`benchmarks/frust_bench/src/scenarios/mod.rs`'s `Scenario` trait ≈81–89;
described in `benchmarks/PROTOCOL.md` §8, "Scenarios (S1–S8)"):

S1 animation storm (bubblebench workload) · S2 long-list scroll (10k rows) ·
S3 table ops · S4 heavy-work responsiveness (50MB JSON parse) · S5 image
pipeline · S6 text-shaping stress · S7 cold start + idle · S8 plugin-call
overhead.

`frust_bench`'s `SCENARIOS` array (`mod.rs` ≈121, the default `db`-feature-on
build) is ten entries: the paired S1–S8 above, plus two DB op-latency
scenarios in a second id namespace, `d1`/`d2` (`benchmarks/PROTOCOL.md` §9,
not §8, and never entered in `RESULTS.md`) — **d1** (an N-row batched insert
inside one transaction, repeated, plus M single-row autocommit inserts) and
**d2** (a point `SELECT` by primary key, repeated, plus one range scan), each
with a byte-identical Rust/Dart Flutter counterpart (`flutter_bench/lib/
scenarios/d1_db_write.dart` is a bit-for-bit port). Unlike S1–S8's shared
percentile-stats script, d1/d2 report through §7's per-op
`frust-perf op scenario=<id> op=<name> n=.. us=.. err=..` line contract — one
line per DB call, not a frame series. A `--no-default-features` build drops
the `db` feature and both `d*` scenarios, leaving the eight-scenario
`SCENARIOS` array (`mod.rs` ≈137) the app-size matrix builds.

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
defaults to huddle).

## Experiments

### 8.1 — Your first raw capture + histogram

```bash
cd benchmarks/frust_bench
FRUST_TRACE=1 FRUST_TRACE_RAW=1 cargo run 2>&1 | tee /tmp/bb.raw
```

Interact for ~30s, quit, then slice it — the fields are `key=value`, so
even a one-liner works:

```bash
awk '/frust-perf raw/ {for(i=1;i<=NF;i++) if($i ~ /^paint_us=/){sub("paint_us=","",$i); print $i}}' /tmp/bb.raw | sort -n | awk '{a[NR]=$1} END {print "p50",a[int(NR*.5)], "p95",a[int(NR*.95)], "p99",a[int(NR*.99)]}'
```

(The harness's `stats.py` under `benchmarks/harness/` does this properly —
read it once.) Now re-run every experiment you eyeballed earlier with real
percentiles: gradient→solid (lab 2.1), bubble count sweep, lab 4.2's capability-gate
probe (there is no CPU-tier comparison arm any more — the engine is the only
renderer).

### 8.2 — Decompose a cold start

`FRUST_TRACE=1 (cd examples/huddle && cargo run)` and read the one
`frust-perf startup` line. Which dominates on your machine —
`device_ready` (wgpu adapter+device) or `first_encode_done` (the engine's
first scene-to-GPU encode)? Match each milestone to the code that stamps it —
`adapter_ready`/`device_ready`/`renderer_ready`/`pipeline_cache_restored` all
come from one `persist_and_record` call
(`crates/frust-shell-desktop/src/render.rs` ≈724, run right after surface
creation on both the inline and render-thread-split paths — chapter 3);
`first_encode_done`/`first_frame_presented` are stamped inside the shared
`render_frame` helper (`render.rs` ≈839/886). This is the map you'll need the
day cold start regresses.

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

`mark_scenario_start/end` (`perf.rs` ≈1228–1239) is public shell-common API.
Wrap something you care about in `frust_bench`'s S1 scenario (e.g. the first
10s after launch vs. after a Reset) and slice your raw log by the marker
lines. You
now have the same tooling the S1–S8 suite uses, on your own workload.

## Where to go from here

- The per-pass numbers name their own next chapter: high `paint_us` → your
  widgets (ch. 2); high `encode_us` → command volume / the engine's compiler
  and passes (ch. 11, 13); high `rebuild_us` → view diffing and signal graph
  (ch. 3); `skipped` anomalies → the gate (ch. 7).
- When a question outgrows this repo's instruments (per-shader GPU timings),
  lab 5.4's Metal capture / RenderDoc is the escalation path — and by then
  the external roadmap's profiling section will read as a tool manual, not
  theory.
