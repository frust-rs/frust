# Frust vs Flutter Benchmark Protocol

This document is the published methodology for the paired benchmark suite
(`benchmarks/frust_bench`, `benchmarks/flutter_bench`, `benchmarks/harness`)
built by Phase 9.E of `workflow/plans/features/frust-phase-9-rust-advantage/PLAN.md`.
It is transcribed from that plan's Phase 9.E, step 1 (binding) plus the
supporting corrections in `research/RESEARCH.md` §7/§8 — nothing below is
invented beyond what is explicitly marked as *our declared convention* (no
industry-standard cross-framework benchmark protocol exists; see the
refuted-claims ledger in `research/RESEARCH.md`).

Both apps' full source ships in this tree for scrutiny. Every number in
`RESULTS.md` must be reproducible by re-running `./benchmarks/harness/run.sh`
against the same device.

## 1. Device matrix

| Column | Device | Chipset | Status |
|---|---|---|---|
| Mid-tier Android | OnePlus 9 (LE2115) | Snapdragon 888 / Adreno 660 | **at desk 2026-07-20** |
| iOS | iPhone SE | Apple A-series (ProMotion N/A — 60Hz panel) | **at desk 2026-07-20** |
| Headline Android | OnePlus 15 | Snapdragon 8-series / Adreno 840 | **pending availability** — added to the matrix (and `RESULTS.md`) whenever this device is next at desk |

`RESULTS.md` records whatever devices actually ran a given matrix pass, not
an aspirational list — a device column with no runs is left absent, not
filled with placeholder numbers. Any device swapped in later must minimally
match this table's chipset-tier shape (one mid-tier Android, one headline
Android with a fast GPU, one iPhone) for a result to be comparable against
history.

### Physical-iPhone automation (iOS 17+, `devicectl`)

`benchmarks/harness/run.sh --platform ios` drives a physical iPhone via
`xcrun devicectl` (validated on the iPhone SE, 2026-07-21). There is **no
CLI deep-link trigger** for a physical iOS device, so scenario selection and
trace capture use a different mechanism per app — both fully automatic (no
manual taps), both documented in the harness header:

- **Frust — runtime env var, one signed build.** `devicectl device process
  launch -e '{"FRUST_BENCH_SCENARIO":"<sN>",...}'` delivers the env var to
  the process; the bench app's existing `resolve_initial()` fallback reads it
  for a **cold** scenario selection (no warm-switch flash — matters for S7's
  cold start). The app's raw `frust-perf`/`bench-scenario` stdout is captured
  live off `--console`. `FRUST_TRACE`/`FRUST_TRACE_RAW` are baked at
  compile time via `frust build ios --define` (the generated iOS build
  run-script exports `FRUST_DEFINES` before `cargo build`, so `option_env!`
  reads them).
- **Flutter — compile-time define, one profile build per scenario.** Dart's
  `String.fromEnvironment('SCENARIO')` is compile-time only, so each scenario
  is a separate `flutter build ios --profile --dart-define=SCENARIO=<sN>`
  (8 builds). Flutter's `print` routes to os_log, which neither
  `devicectl --console` nor the (empty on modern iOS) legacy syslog relay
  surfaces, so `flutter_bench` also writes its trace to a file in its `tmp/`
  container that the harness pulls with `devicectl device copy from
  --domain-type appDataContainer` after each run. Both transports are sliced
  by the same `stats.py` from identical `*-perf raw`/`bench-scenario` lines,
  so the transport choice affects no computed number.

Two metric caveats are iOS-specific and recorded per-run in `RESULTS.md`:
Flutter's `FrameTiming.totalSpan` is unreliable (negative under load) on iOS,
so its frame total is reported as the `build+raster` work-sum; and Frust's
`total_us` folds in the CADisplayLink present-to-vsync idle wait, so its
per-frame percentiles are a cadence wall-time, not a pure render cost. iOS
also exposes no brightness/battery/thermal or external-cold-start/CPU/RSS CLI,
so those environmental controls and S7 metrics are uncontrolled/unavailable
(cooldown is a fixed inter-scenario wait).

## 2. Framework build-config asymmetry (declared and justified)

- **Flutter runs in profile mode.** Flutter's own documentation requires
  this for meaningful measurement: debug mode carries JIT compilation and
  assertions (both invalidate timing), release mode strips the service
  extensions the framework's own tracing/timeline tooling depends on, so
  it cannot be traced at all. Profile mode is Flutter's documented
  stand-in for "release-like performance, traceable."
- **Frust runs in `--profile`.** Like Flutter, Frust now has a dedicated
  profile mode for on-device measurement. The `--profile` build uses
  release-level optimizations (fat LTO, `codegen-units = 1`, `strip =
  "symbols"` at link time) with debug symbols retained, and with the
  `perf-trace` feature enabled at compile time so `FRUST_TRACE`/`FRUST_TRACE_RAW`
  instrumentation is compiled IN (not runtime-gated or dormant). This gives
  **both frameworks measured in their respective profile modes** — a fairness
  improvement: Frust is now measured under conditions closer to release
  performance (release optimization profile) while keeping instrumentation
  compiled in, exactly as Flutter's profile mode does. See the METHODOLOGY
  BREAK section below for comparability notes.

## 2.5 Methodology Break (2026-07-23, post–Phase-11.A)

**Results recorded before 2026-07-23 are not directly comparable with results
recorded after without an A/B measurement to bound the delta.**

Prior to this protocol update, Frust benchmark scenarios ran in `--release`
mode with instrumentation compiled in but runtime-dormant (environment checks
bypassed the logging at call sites). Results captured under that configuration
(dormant-instrumentation release builds) reflected release-mode performance
with instrumentation overhead compiled in but not running.

**As of 2026-07-23**, Frust benchmark scenarios run in `--profile` mode (the
same mode used for development/debugging). This build uses release-level
optimizations but with instrumentation (`FRUST_TRACE`/`FRUST_TRACE_RAW`)
compiled in and **active** (not runtime-dormant). The instrumentation activity
may introduce measurable timing overhead compared to a dormant-instrumentation
baseline — this is analogous to the documented Flutter profile-vs-release
difference (§2).

**Comparability:** To quantify the release-vs-profile delta on Frust and ensure
results are interpreted correctly, **task 09 (device A/B gate)** runs S1 + S3
scenarios in both modes on a controlled device (OnePlus 9), recording both
release and profile measurements side-by-side. That bounded delta is then
documented in `RESULTS.md` as a deviation note for the associated run. All new
results going forward use `--profile` and should not be directly compared
against pre-2026-07-23 baselines without reference to the task-09 delta.

## 3. Environmental controls

Applied identically to both apps, every run, every device:

- Fixed screen brightness (recorded per device in `RESULTS.md`).
- Airplane mode on (no network/radio jitter).
- Charger disconnected (thermal/DVFS behavior under battery, not
  plugged-in boost).
- A thermal cooldown pause between scenario runs — device screen off,
  idle, until surface temperature (or, lacking a sensor reading, a fixed
  wait — recorded in `RESULTS.md`) returns to a baseline reading recorded
  once per device at the start of a matrix pass.

## 4. Run count and warmup (our declared convention)

**No industry-standard convention exists for run count, warmup discard, or
run duration in cross-framework UI benchmarking** (RESEARCH.md §8's
corrections). This suite declares and uses:

- **≥10 runs × 30 seconds** per scenario per app per device.
- **The first 2 runs of each set are discarded** (JIT/cache/thermal
  warmup) before computing statistics.
- All discarded and kept runs' raw series are still committed to
  `benchmarks/raw/` alongside the computed table, so a reader can verify
  the discard didn't cherry-pick.

This diverges from Flutter's own perf-testing guidance (~100 runs,
RESEARCH.md §7) for practicality on a person-driven physical-device matrix;
the divergence is recorded, not hidden.

## 5. Statistics — one shared script, both sides

`benchmarks/harness/stats.py` (or equivalent — see `benchmarks/harness/`)
is the **single** script that computes every published percentile, for
both apps, from their respective raw per-frame series. Neither app's own
in-framework summarizer (Flutter's `TimelineSummary`, Frust's
`frust-perf frame` ~2s aggregate) is used for the headline numbers — both
export a raw series instead (see §7) so the exact same nearest-rank
percentile math runs over both.

Per scenario, per app, per device, the harness reports:

- **p50 / p95 / p99 / worst** total frame time (build+raster combined for
  Flutter's `FrameTiming`; `total_us` for Frust's raw line).
- **Missed-budget counts at both 16.67ms (60Hz) and 8.33ms (120Hz)** —
  every device that supports a 120Hz mode is measured against both
  budgets; a 60Hz-only panel (e.g. this suite's iPhone SE) is measured
  against 16.67ms only and the 8.33ms column is reported as N/A rather
  than a misleading synthetic figure.
- Per-scenario additional metrics where the scenario table (§8) calls for
  them (e.g. S3's per-op timings, S4/S8's latency + wall-time, S7's
  startup spans + idle CPU/memory).

## 6. Fairness gates

Both apps must clear these before a run counts toward `RESULTS.md`:

- **Android high-refresh opt-in.** Flutter does not call
  `Surface.setFrameRate` by default (RESEARCH.md §7) — the Flutter bench
  app must wire the equivalent opt-in explicitly (plugin or platform
  channel) wherever the device supports >60Hz, exactly mirroring Frust's
  own `Surface.setFrameRate` hint (`docs/spec.md`/`docs/ARCHITECTURE.md`'s
  High refresh-rate hints).
- **iOS `CADisableMinimumFrameDurationOnPhone`.** The Flutter bench app's
  Info.plist must set this, mirroring Frust's `CADisplayLink`
  `preferredFrameRateRange` request, so neither app is artificially capped
  below the panel's real refresh rate.
- **Identical logical work.** Same random seed, same dataset bytes (the
  10k-row dataset, the JSON payload, the image set), same scripted input
  timeline (fling velocity/timing, scroll distance, tap sequence) fed to
  both apps by the harness, not hand-tuned per framework.
- **Idiomatic code on both sides.** The Flutter app follows Flutter's own
  documented performance best practices (const constructors,
  `RepaintBoundary` where the Flutter team's own guidance calls for one,
  `ListView.builder` for virtualization, etc.) — it is not deliberately
  written to lose. The Frust app follows this repo's own idioms
  (`docs/CODE_STANDARDS.md`) with no benchmark-only shortcuts undocumented
  in its own source. **The Flutter app's full source is published in this
  tree** for exactly this reason — a claimed unfairness must point at a
  line of code, not an assertion.

## 7. Raw-series export formats (exact, shipped format)

### Frust: `frust-perf raw` (per frame) and `bench-scenario-*` (markers)

Emitted by `crates/frust-shell-common/src/perf.rs` when both `FRUST_TRACE`
and `FRUST_TRACE_RAW` are set (see `docs/DEVELOPMENT.md`'s Instrumentation
table). One line per recorded frame, microsecond resolution, in this exact
field order (`format_raw_frame_line`):

```
frust-perf raw n=<u64> total_us=<u128> rebuild_us=<u128> layout_us=<u128> paint_us=<u128> encode_us=<u128> acquire_us=<u128> submit_us=<u128> skipped=<0|1>
```

> **Raw-format change — v3, 2026-07-22 (Phase 11.A).** The single `present_us`
> field of v2 was split into separate `acquire_us` + `submit_us` fields so the
> blocking swapchain-acquire (vsync) wait is attributable separately from the
> blit/queue-submit work — the S5 GPU-saturation-vs-blit-cost question. No
> combined field is kept. **Cross-device comparability:** a v2 log's
> `present_us` corresponds to `acquire_us + submit_us` in v3 — sum the two when
> comparing a post-split capture against a v2 (2026-07-21…) baseline.
>
> **Raw-format change — v2, 2026-07-21 (Phase 10.A).** The single
> `encode_present_us` field of v1 was split into separate `encode_us` +
> `present_us` fields so GPU/CPU encode cost and the swapchain-acquire
> (vsync) wait are separately attributable (the render-thread-split GO/NO-GO
> decision is made on the encode-only number). A v1 log's `encode_present_us`
> corresponds to `encode_us + present_us` in v2 (== `encode_us + acquire_us +
> submit_us` in v3). `stats.py`'s parsing is key=value and forward-compatible,
> so its cross-app percentile table (keyed on `total_us`) is unaffected by
> either split.

- `n` — 1-indexed running frame counter.
- `total_us` — whole-frame duration
  (rebuild+layout+paint+encode+acquire+submit).
- `rebuild_us` / `layout_us` / `paint_us` / `encode_us` / `acquire_us` /
  `submit_us` — per-pass durations, microseconds. `encode_us` is the GPU/CPU
  encode span (no vsync wait); `acquire_us` is the swapchain-acquire span
  (dominated by the blocking vsync wait); `submit_us` is the blit +
  queue-submit + present span. `acquire_us + submit_us` equals the old v2
  `present_us`.
- `skipped` — `1` if the frame-gate skipped this tick (see
  `docs/ARCHITECTURE.md`'s Frame gate), else `0`. A skipped frame's other
  fields are near-zero and should be excluded from percentile math the
  same way a scenario harness excludes idle/no-op frames on the Flutter
  side.

Scenario boundaries (`mark_scenario_start`/`mark_scenario_end`, same
`FRUST_TRACE`+`FRUST_TRACE_RAW` gating) stamp:

```
bench-scenario-start <name>
bench-scenario-end <name>
```

into the same log stream, letting the harness slice the per-frame series
by scenario without any other coupling to the app.

### Flutter: required equivalent

`benchmarks/flutter_bench` (task `05-flutter-bench`) MUST emit a parseable
line per `FrameTiming`, captured via
`SchedulerBinding.addTimingsCallback` (works in profile mode on-device
with no host test runner — RESEARCH.md §7), in a format the same harness
can parse with a symmetrical field set:

```
frust-bench raw n=<u64> total_us=<u64> build_us=<u64> raster_us=<u64> skipped=0
```

- `total_us` — `FrameTiming.totalSpan.inMicroseconds`.
- `build_us` — `FrameTiming.buildDuration.inMicroseconds` (Flutter's
  nearest equivalent to Frust's `rebuild_us`+`layout_us`+`paint_us`
  combined — Flutter does not expose a rebuild/layout/paint split the way
  Frust's `FramePasses` does, so this is reported as one combined figure,
  not force-split to match Frust's finer granularity).
- `raster_us` — `FrameTiming.rasterDuration.inMicroseconds` (Flutter's
  nearest equivalent to Frust's `encode_us + acquire_us + submit_us`
  combined; Flutter's raster span does not separate encode from
  acquire/submit the way Frust's v3 raw line now does).
- `skipped` — always `0` today; reserved for parity with Frust's frame
  gate if the Flutter app ever gains an analogous skip path.

and identical scenario markers:

```
bench-scenario-start <name>
bench-scenario-end <name>
```

The harness (`06-harness`) treats `total_us`/`skipped`/the marker pair as
the common contract across both formats; `rebuild_us`/`layout_us`/
`paint_us` vs `build_us`/`raster_us` are reported as each framework's own
native pass breakdown, not force-unified into a single column.

## 8. Scenarios (S1–S8)

Verbatim from `PLAN.md` Phase 9.E (each app implements all eight, selected
via a launch arg / deep link so one binary drives the whole matrix):

| ID | Scenario | What it stresses | Rust-advantage claim under test |
|----|----------|------------------|-------------------------------|
| S1 | **Animation storm** — the bubblebench workload (gradient bubbles + text, perpetual physics; bubble sizes normalized to the play area, count derived for ~50% area coverage of the safe-area play region, cap 60 — settle-capable field, user-calibrated 2026-07-20 — see S1-specific notes) | per-frame paint, gradient/state-change behavior | vello single-dispatch vs Impeller per-primitive state churn (proven on Adreno 840; now formalized) |
| S2 | **Long-list scroll** — 10k rows, text + thumbnail + icons, scripted fling + steady scroll | virtualization, reconciliation, shaping cache | no-GC frame stability at 120Hz |
| S3 | **Table ops** — js-framework-benchmark subset: create 1k, update every 10th of 10k, swap, clear (scripted, timed per op); the sequence **cycles continuously** for the whole capture window (a fixed settle-gap constant between ops and after `clear` before the next cycle's `create1k` — canonical in `flutter_bench/lib/bench/datasets.dart`'s `s3SettleGapMs`), with per-op `s3-*` stats aggregated by the harness across all cycles' occurrences of each marker name | build/diff/reconcile throughput | view-diff + arena rebuild vs Element tree |
| S4 | **Heavy-work responsiveness** — parse a ~50MB JSON (or synthetic equivalent) while an animation runs; measure animation percentiles during the work + total wall time | the heavy-work idiom end-to-end | `use_task`+`spawn_blocking` (move) vs `compute()`/`Isolate.run` (copy) |
| S5 | **Image pipeline** — decode-and-display stream of large images while scrolling | off-thread decode, zero-copy handoff | `decode_async` Arc move vs isolate transfer |
| S6 | **Text shaping stress** — multilingual long-paragraph relayout on width animation | Parley vs Flutter text pipeline | shaping cost + cache behavior |
| S7 | **Cold start + idle** — launch to first frame (each framework's own spans + external `am start -W`); then 60s idle CPU + sustained memory | startup, idle cost | thin runtime, frame-gate idle |
| S8 | **Plugin-call overhead** — shared_preferences write+read loops (N unique keys, all five value types), latency per op + total wall time; a burst variant runs during an S1-style animation to measure UI-thread impact | the plugin boundary itself | direct in-process FFI (`objc2`/`jni`, zero codec) vs MethodChannel round-trip (StandardMethodCodec serialize → channel hop → Kotlin/Swift wrapper → and back) |

### S1-specific notes

- **Bubble sizes are normalized to the play area** (v2): bubble radius and
  initial cluster spread are fractions of `min(playWidth, playHeight)` rather
  than absolute logical pixels, so the bubble-size-to-play-area ratio (and
  thus collision density / settle behavior) is identical across the two apps
  regardless of each shell's reported logical size. The **play area = the
  safe-area-inset region on both sides** (Flutter hosts under
  `Scaffold > SafeArea`; the frust app wraps its scenario host in
  `SafeArea`).
- **Bubble count derived for ~50% area coverage of the safe-area play region,
  cap 60 — settle-capable field, user-calibrated 2026-07-20 (v3).** v2's
  fixed 60-bubble count, even at fractional radii, still summed to more than
  a phone's play area, so the field stayed jam-packed with constant
  collisions and never settled (confirmed on-device). v3 instead derives the
  bubble **count** at scenario init — `N = floor(0.5 * playW * playH /
  E[bubbleArea])`, `E[bubbleArea]` computed analytically (not sampled) over
  the v2 radius distribution — capped at the original 60 so a wide desktop
  window doesn't explode the count. Radii, seeds, and the velocity/physics
  constants stay byte-identical between the two apps, and the per-bubble RNG
  draw order (performance, radius, x, y) is unchanged, so `N` only truncates
  the same deterministic sequence — an identical play area yields an
  identical `N` on both apps. The canonical spec lives in
  `flutter_bench/lib/bench/datasets.dart` (mirrored in the frust side's
  `s1_animation/physics.rs`).

### S5-specific notes

- **Cell layout parity is v2 (2026-07-21).** The reference geometry is what
  Flutter *actually renders*: a vertical `ListView` imposes tight cross-axis
  constraints, overriding `_ImageCell`'s `SizedBox(width: 256)` — each image
  paints `(viewportWidth − 16) × 256` (`BoxFit.cover`), edge-to-edge. The
  frust scenario originally painted the literal 256×256 (≈66% width),
  compositing ~⅓ fewer pixels per frame; from v2 it mirrors the full-width
  geometry (`(viewport.width − 2·pad) × 256`, `ImageFit::Cover`). S5 numbers
  captured before this date are not cross-app comparable and were retired
  with the 2026-07-21 results reset.

### S7-specific notes

- Frust's own `StartupSpans` (`native_lib_load`, `init_entry`,
  `adapter_ready`, `device_ready`, `renderer_ready`, `first_rebuild_done`,
  `first_frame_presented` — `frust-perf startup ...`, ms resolution) are
  reported alongside the platform-external `am start -W`
  ("TotalTime"/"WaitTime") figure for Android, and the equivalent
  external cold-start measurement for iOS. Flutter reports its own
  framework-level first-frame timeline event plus the same external
  `am start -W` figure, so at least one measurement (the external one) is
  taken with an identical tool on both sides.
- Idle CPU/memory: 60 seconds with no input after first frame, sampled via
  the platform's own tools (`adb shell dumpsys cpuinfo`/`meminfo` on
  Android; Instruments or `xcrun` equivalents on iOS). Memory deltas are
  compared **within a platform only** (Android PSS vs iOS RSS are not
  directly comparable units — RESEARCH.md §8) — no cross-platform memory
  column is published.

### S8-specific fairness rules

- Both sides use their framework's **idiomatic** preferences API: the
  Flutter `shared_preferences` pub package vs Frust's
  `frust-shared-preferences` (`plugins/shared-preferences`) — no
  hand-rolled bypass on either side.
- **Flutter's `shared_preferences` package caches reads in Dart memory**
  after the first load of a key — a naive read-loop benchmark would
  measure Dart-side map lookups, not the MethodChannel boundary the
  scenario claims to test. This scenario therefore separately measures
  and reports **both**:
  - **Cached-read latency** (subsequent reads of an already-loaded key —
    Dart memory lookup on Flutter's side, a real in-process call on
    Frust's, since `frust-shared-preferences` has no such cache layer).
  - **Channel-crossing latency** (writes, which always cross the channel
    on Flutter's side regardless of caching; and reads that force a
    channel crossing — unique per-iteration keys and/or exercising the
    package's reload/`getInstance()` path).
- **Writes always cross the channel on Flutter's side** — write latency
  is this scenario's headline number specifically because it cannot be
  short-circuited by Dart-side caching on either framework.
- The burst-during-animation variant reports the S1-style animation's
  frame percentiles *during* the S8 write/read burst, isolating UI-thread
  impact of the plugin boundary from raw call latency.
- **The claim under test is call/boundary latency, not on-disk storage
  format.** `frust-shared-preferences` and Flutter's `shared_preferences`
  package deliberately use different storage encodings by design (see
  the plugin plan's Design Decision 4) — S8 never compares file formats,
  only round-trip call latency for equivalent logical operations across
  all five value types (`bool`/`i64`/`f64`/`String`/`Vec<String>`).
- **Errors are counted, not swallowed.** Both sides carry an `err=0|1`
  field per op line and count write failures plus round-trip-verification
  failures (an unexpectedly-absent key or a value not matching what was
  written) into a `write_errors`/`read_unexpected_none`/`read_value_mismatch`
  tally, emitting one `s8-errors` marker line when any is nonzero so a run
  with a silent boundary failure is flagged rather than reported as a clean
  latency number.

## 9. Reporting

`RESULTS.md` (this directory) is the per-device × per-scenario results
template this protocol feeds. Every published number must:

- Be reproducible via `./benchmarks/harness/run.sh <scenario> --device <serial>`.
- Have its raw series committed under `benchmarks/raw/` alongside the
  computed table.
- Include a methodology-deviations note in `RESULTS.md` if any control in
  this document (environmental, run count, fairness gate) could not be
  applied exactly as specified for that run (e.g. no thermal sensor
  reading available, a device lacking a 120Hz mode, a scenario that
  timed out).

Losses are reported the same as wins — a scenario where Flutter measures
better is published in `RESULTS.md`, not omitted.
