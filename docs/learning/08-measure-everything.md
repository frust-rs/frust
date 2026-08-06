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

**Summary** (default), every ~2s of frames (`perf.rs` ≈305–329 the
`FrameSummary` shape, `emit_log` ≈536–569): p50/p95/p99 totals, per-pass p95s
(`rebuild/layout/paint/encode/acquire/submit` — chapter 3's encode→acquire→submit
split), 60Hz/120Hz budget-overrun counts, `skipped`.

**Raw v3** (`FRUST_TRACE=1 FRUST_TRACE_RAW=1`), one line per frame,
formatted at `perf.rs` ≈624–636 (v3 split the old v2 `present_us` into
`acquire_us`+`submit_us` — see that function's doc comment for the full
v1→v2→v3 history):

```
frust-perf raw n=<u64> total_us=.. rebuild_us=.. layout_us=.. paint_us=.. encode_us=.. acquire_us=.. submit_us=.. skipped=<0|1>
```

plus `bench-scenario-start/end <name>` markers (`perf.rs` ≈507–535) and
free-form `bench_emit` lines (≈549) for per-op scenario measurements.

## The harness (`benchmarks/`)

Paired Frust/Flutter apps implementing the same eight *timed* scenarios
(`benchmarks/frust_bench/src/scenarios/mod.rs` ≈83–94; described in
`benchmarks/PROTOCOL.md` §8, "Scenarios (S1–S8)"):

S1 animation storm (bubblebench workload) · S2 long-list scroll (10k rows) ·
S3 table ops · S4 heavy-work responsiveness (50MB JSON parse) · S5 image
pipeline · S6 text-shaping stress · S7 cold start + idle · S8 plugin-call
overhead.

`frust_bench`'s `SCENARIOS` array (same `mod.rs` anchor, ten entries now) adds
two more that sit outside that paired eight, both excluded from
`PROTOCOL.md`'s §8 and never entered in `RESULTS.md`:

S9 terminal grid stream (replays a checked-in byte fixture through a real
`vt100` VT emulator, batched into an 80×45 character grid — a
candidate-versus-known-good measurement against Flutter's kterm 1.5.3, but
paired only at the *protocol* level today; the Flutter side lives on a spike
branch, not in this tree) · S10 IME keystroke probe (a **capability probe**,
not a measurement — no Flutter counterpart, nothing timed: it recovers
per-keystroke bytes from the mobile IME bridges' whole-state-sync snapshots
via a sentinel-buffer diff, and the deliverable is a readable device log, not
a number).

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
percentiles: gradient→solid (lab 2.1), bubble count sweep, `cpu` tier
(lab 4.2 — compare `encode_us` distributions GPU vs CPU).

### 8.2 — Decompose a cold start

`FRUST_TRACE=1 (cd examples/huddle && cargo run)` and read the one
`frust-perf startup` line. Which dominates on your machine —
`device_ready` (wgpu adapter+device) or `first_encode_done` (first vello
pipeline compile)? Match each milestone to the code that stamps it —
`adapter_ready`/`device_ready`/`renderer_ready`/`pipeline_cache_restored` all
come from one `persist_and_record` call
(`crates/frust-shell-desktop/src/render.rs` ≈723–745, run right after surface
creation on both the inline and render-thread-split paths — chapter 3);
`first_encode_done`/`first_frame_presented` are stamped inside the shared
`render_frame` helper (`render.rs` ≈801/842). This is the map you'll need the
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

`mark_scenario_start/end` (`perf.rs` ≈507–535) is public shell-common API.
Wrap something you care about in `frust_bench`'s S1 scenario (e.g. the first
10s after launch vs. after a Reset) and slice your raw log by the marker
lines. You
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
