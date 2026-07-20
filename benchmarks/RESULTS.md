# Benchmark Results

Template for the paired Frust-vs-Flutter benchmark suite's results, per
`PROTOCOL.md`. This file starts empty (no runs yet) — task `07-run-matrix`
fills in device sections as scenarios are actually run. **A device or
scenario with no completed runs is omitted, not filled with placeholder
numbers.**

Every table below must be reproducible from `benchmarks/raw/<device>/<scenario>/`
(raw per-run series, committed alongside this file) via
`./benchmarks/harness/run.sh <scenario> --device <serial>`.

## Device: OnePlus 9 (LE2115, Adreno 660) — mid-tier Android

**Status:** run 2026-07-20 (short-form: 5 runs × 30s, first 2 discarded — see
run-count deviation). **Frust = RELEASE build; Flutter = profile (per protocol
§2).** S2/S4/S5/S6/S7 below are the `ade0d87` release re-run (unaffected by
the later S1/S3 spec changes); S1 and S3 are a further re-run from commit
`73fa17f` (S1 spec v3 count-derivation + S3 continuous op cycling — both apps
rebuilt fresh, keystore/trace-env recipe unchanged from `ade0d87`); **S8 is a
further, final re-run from commit `bfd6fd4` (2026-07-21)** — the S8
error-accounting + read-timing-symmetry fixes (see S8's own banner below).
Raw series under `raw/oneplus9/frust_release/<scenario>/runN.log` (Frust
release; S1/S3 additionally under `.../s1_final/` and `.../s3_final/`, S8
under `.../s8_reval/`, for each's final re-run) and
`raw/oneplus9/<app>/<scenario>/runN.log` (Flutter profile, same
`s1_final`/`s3_final`/`s8_reval` subfolders; gitignored — full-device logcat is
not committed).

- Chipset: Snapdragon 888 / Adreno 660
- OS version: Android 15 (build.version.release 15)
- Fixed brightness: 128 / 255 (auto-brightness disabled)
- Refresh-rate mode used: 120Hz panel (device advertises 120Hz + 60Hz modes;
  both apps opt into high refresh — Frust `Surface.setFrameRate`, Flutter
  `flutter_displaymode`). Achieved rate is device/thermal-dependent; both
  budgets (16.67ms/8.33ms) are therefore reported.
- Flutter version: 3.44.2 stable — built `flutter build apk --profile`
  (PROTOCOL §2: Flutter's documented traceable "release-like" mode).
- Frust build: **RELEASE** (`frust build apk --release` — fat LTO,
  `codegen-units=1`, `strip=symbols`, `panic=abort`), signed with the same
  Android upload keystore `examples/huddle` uses (copied `key.properties` with an
  absolute `storeFile`; keystore not moved; `key.properties` gitignored). Repo at
  commit `ade0d87` (the S2/S5 scroll-relayout fix). `FRUST_TRACE=1
  FRUST_TRACE_RAW=1` — **note:** in a release build Gradle's `--define`
  env-injection did not reach the cargo compile, so the two dials were baked by
  exporting them in the build shell (cargo-ndk inherits the env; `option_env!`
  reads it). Confirmed emitting `frust-perf raw`/`startup`.
- Thermal-cooldown method: gate-based (`device_state.sh`, `dumpsys battery`
  temperature, ceiling 38°C, proceed-if-under) before each scenario block, with
  the screen slept between blocks. Observed ~30–31°C at every block start — no
  forced cooldown wait ever triggered (no fixed idle pause beyond the gate).
- Charger: device physically USB-connected (required for adb); `dumpsys battery
  unplug` presented an on-battery state to the DVFS/thermal governor identically.
  Airplane mode on; Wi-Fi disabled. `svc power stayon true` was set for the
  release run (a battery-unplug side effect re-enabled the screen timeout, locking
  the screen between scenarios; the runner also wakes + dismisses keyguard at each
  block start — see the taint-audit note in deviations).

### S1 — Animation storm

> **RESOLVED — 2026-07-20 final re-run under S1 spec v3 (commit `89ca8e8`).**
> The parity chain that produced the PARITY-SUSPECT banner above is now
> fully closed: v1's absolute-pixel bubble sizes → v2's fraction-of-play-area
> sizes (commit `166812b`) → v3's count-derived-for-~50%-coverage field
> (commit `89ca8e8`, user-calibrated after v2 still visually over-packed on
> device). Both apps rebuilt fresh from commit `73fa17f` (HEAD at run time;
> includes v3 + the S3 continuous-cycling commit, S1-irrelevant) and re-run:
> **Frust = `--release`** (huddle keystore, `FRUST_TRACE`/`FRUST_TRACE_RAW`
> exported at build time + a `perf.rs` mtime touch to force the recompile —
> confirmed emitting `frust-perf raw`/`bench-scenario-start s1` on-device
> before scoring), **Flutter = `flutter build apk --profile`** (3.44.2
> stable, confirmed emitting `flutter-perf raw` before scoring). 5 runs ×
> 30s each app, first 2 discarded (short-form — see run-count deviation),
> interleaved with S3 below. A winner is declared this time — see below.

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames (90s, 3 kept runs) |
|---|---|---|---|---|---|---|---|
| Frust (release, v3) | 9.75 | **12.41** | **15.79** | **55.60** | **52 (0.9%)** | 4489 (77.2%) | 5812 (~64.6 fps avg) |
| Flutter (profile, v3) | **9.27** | 16.87 | 19.25 | 38.60 | 459 (5.1%) | **7872 (87.5%)** | 9000 (~100.0 fps avg) |

**Split result — no single winner; Frust wins on tail/jank, Flutter wins on
median and raw frame count.** Medians are within noise of each other (Frust
9.75ms vs Flutter 9.27ms), but the two apps diverge sharply past p50: Frust's
p95/p99/worst are all tighter (12.41/15.79/55.60ms vs Flutter's
16.87/19.25/38.60ms — Frust's single worst outlier is higher, but its whole
p95+ band sits lower), and Frust drops the 60Hz budget far less often, both
in absolute count (52 vs 459) and as a share of frames rendered (0.9% vs
5.1% — **Flutter drops the 60Hz budget ~5.7× more often per frame attempted**).
Flutter's higher missed-120Hz *share* (87.5% vs 77.2%) is a direct consequence
of rendering far more total frames (100fps avg vs Frust's ~64.6fps avg) at the
same workload — pushing more frames into the tight 8.33ms budget's reach in
the first place, not necessarily worse per-frame cost. Net honest read: at
this settle-capable, area-matched workload, **Frust is the smoother/less
janky renderer (fewer, more consistent budget misses)**; **Flutter achieves a
marginally faster typical frame and a higher sustained frame rate**. Framing
this as a clean win either way would overstate the data — recorded as a split,
consistent with this doc's existing convention for S4/S5.

**Settled-tail handling (as requested — no post-settle idle observed this
pass):** neither app's kept runs recorded a *single* `skipped=1`/idle frame
(0/5812 Frust, 0/9000 Flutter) — the field never reached
`is_fully_settled`/its Flutter equivalent within any 30s capture window at
this device's phone-sized play area and ~50%-coverage bubble count (dense
enough that collisions keep re-energizing the field faster than
`FRICTION=0.90`/frame damps it out, per `physics.rs`). This is a genuine,
honest finding, not a missing measurement: **there is no settled tail to
report in this dataset.** The mechanism that would handle one if it appeared
is already in place and already exercised elsewhere in this file (S3, below,
and the frust-side `FrameStats::record` contract) — a frame the mobile
frame-gate skips still emits a full `frust-perf raw ... skipped=1` line
(all-zero pass durations), so `stats.py`'s `compute_stats` counts it in
`n_total`/`skipped` while excluding it from the percentile/`worst` math
(mirroring `FrameStats::summary`'s own semantics) — an absent/idle frame is
never silently dropped from the reported totals, only from the latency
distribution it would otherwise (and wrongly) drag toward zero. If a future
longer capture or a smaller/denser play area does settle mid-window, the same
`n_total`/`n_active`/`skipped` triple already reported for every table in
this file (see S3's per-scenario line) is where that would show up.

### S2 — Long-list scroll (10k rows)

Frust re-run post-fix `ade0d87` (release build), health-gated: `layout_us > 0` on
5319/5322 frames (mean 3802µs) — the scroll window now re-lays-out every frame, so
this is a **valid** scroll (unlike the earlier profile pass, which measured a
frozen window and was discarded).

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | 11.04 | 15.52 | 17.91 | 54.42 | 104 | 5157 |
| Flutter (profile) | 15.27 | 24.79 | 27.26 | 90.78 | 666 | 1559 |

**Frust wins S2.** With a genuine scroll, Frust holds ~11.0ms median / 15.5ms p95
vs Flutter's 15.3ms / 24.8ms, with far fewer dropped-60Hz frames (104 vs 666) and
a much shorter worst (54.4ms vs 90.8ms). No-GC frame stability under scripted fling
holds up once the relayout bug is fixed. (Flutter's higher @8.33ms miss count vs
Frust is not directly comparable — Flutter rendered fewer total frames; p50/p95 and
the @16.67ms drops are the primary comparison.)

### S3 — Table ops (js-framework-benchmark subset)

> **RESOLVED (health) — 2026-07-20 final re-run under S3 continuous cycling
> (commit `73fa17f`).** The original SUSPECT finding root-caused to the op
> sequence running once and legitimately going blank at `clear` for the rest
> of the capture window (not rendering occlusion) — see the prior root-cause
> note preserved in git history. Both apps now cycle the five-op sequence
> (`create1k→create10k→update→swap→clear`, 300ms settle gap between ops and
> before each cycle's restart) continuously for the whole window; both
> rebuilt fresh from commit `73fa17f` (same build config as S1 above) and
> re-run, 5×30s, first 2 discarded, interleaved with S1. **Health confirmed
> on both apps**: every kept run shows 18–21 repeats of all five `s3-*`
> sub-markers (`bench-scenario-start s3-create1k` reopening after
> `s3-clear` closes, repeatedly — see counts below), and the table is
> non-empty throughout except the intentional inter-op/inter-cycle settle
> gaps. No visual-glitch/occlusion signature in the raw series. A firm
> per-op winner could not be declared for Flutter this pass — see below.

**Cycle-count health check** (kept runs 3–5, count of `bench-scenario-start
s3-<op>` occurrences per 30s run — confirms continuous cycling, not a
run-once sequence):

| App | create1k | create10k | update | swap | clear |
|---|---|---|---|---|---|
| Frust (release) | 20 / 20 / 21 | 20 / 19 / 21 | 20 / 19 / 21 | 19 / 19 / 20 | 19 / 19 / 20 |
| Flutter (profile) | 19 / 19 / 19 | 18 / 18 / 18 | 18 / 18 / 18 | 18 / 18 / 18 | 18 / 18 / 18 |

**Per-op reconcile-frame timing** (`s3-*` sub-markers, aggregated across all
cycles' occurrences in the 3 kept runs — `stats.py`'s marker-toggle logic
folds every repeat of a marker name into one combined series, verified by
`test_stats.py`'s `test_repeated_marker_pairs_aggregate_across_cycles`):

| Op | Frust p50/p95/p99/worst (ms, release) | n frames | Flutter |
|---|---|---|---|
| create 1k | 12.48 / 24.43 / 62.67 / 62.67 | 61 | **not captured** |
| create 10k | 24.02 / 26.75 / 27.42 / 27.42 | 60 | **not captured** |
| update every 10th of 10k | 19.31 / 22.48 / 24.03 / 24.03 | 59 | **not captured** |
| swap | 17.81 / 20.95 / 22.03 / 22.03 | 58 | **not captured** |
| clear | 17.91 / 20.81 / 21.04 / 21.04 | 58 | **not captured** |

**Flutter per-op capture landed zero frames in any of the five sub-marker
windows, across all three kept runs** (worse than the earlier one-shot
pass, which caught one `create1k` frame by luck) — confirmed by direct
inspection: every `bench-scenario-start s3-<op>` / `bench-scenario-end
s3-<op>` pair in the Flutter logs is immediately adjacent (the end marker
fires on the very next line, before any `flutter-perf raw` line), because
Flutter's `SchedulerBinding.addTimingsCallback` delivers each frame's timing
data asynchronously, on a later event-loop turn — always after the
synchronous marker-close call the op itself makes. This is a structural
instrumentation gap on the Flutter side (marker placement vs callback
delivery timing), not a Frust defect, and not something this task's scope
covers fixing (would need a Flutter-side code change to move the `end`
marker to actually run inside `addTimingsCallback`, out of scope here) —
flagged as a deviation and a follow-up.

**Overall S3 frame series** (marker `s3`, whole capture including settle
gaps): Frust p50 9.02ms / p95 12.44ms / p99 22.44ms / worst 62.67ms (5900
active of 5900 total, 0 skipped, ~65.6 fps avg — `Ticker` stays
`active: true` continuously per the redesign, so Frust paints every tick
including the 300ms settle gaps, not only at op transitions); Flutter p50
19.40ms / p95 34.89ms / p99 39.49ms / worst 40.55ms (275 active of 275
total, 0 skipped, ~3.1 fps avg — Flutter only renders a frame when an op
actually triggers a rebuild, staying idle the rest of the time by its own
event-driven default, no continuous ticker).

**No overall winner declared from the whole-series numbers** — they are not
an apples-to-apples comparison: Frust's series is dominated by cheap,
continuous idle-gap repaints (5900 frames, most doing nothing but redraw a
static table) that pull its percentiles down, while Flutter's sparse series
(275 frames) contains *only* the substantive op-transition frames, which
should if anything read *heavier* per frame, not lighter — the two series
measure different things by construction (continuous-paint vs
event-driven-paint), not a fair reconcile-cost comparison. **The only clean,
comparable numbers this pass are Frust's per-op table above**, which show
reconcile cost rising from ~12.5ms (create 1k) to ~24ms (create 10k, the
heaviest op) and settling to ~18–20ms for update/swap/clear — all comfortably
inside a single 60Hz frame's budget except the create-1k tail
(p99/worst 62.67ms, likely a cold-glyph-cache/JIT-warmup first-occurrence
outlier diluted across cycles, consistent with the desktop smoke test's own
note on `create1k`'s first-cycle cost). No Flutter per-op figure exists to
compare against this pass.

### S4 — Heavy-work responsiveness (~50MB JSON parse + concurrent animation)

Animation-responsiveness metric is the whole-scenario frame series (animation runs
throughout the parse); Frust = release. **Parse wall-time is not recoverable** —
the harness captures logcat `-v raw`, which strips timestamps, and neither app
emits an explicit parse-duration line (deviation).

| Metric | Frust (release) | Flutter |
|---|---|---|
| Total wall time (parse complete) | not captured (see deviations) | not captured |
| Animation p50 during parse (ms) | 4.61 | 4.23 |
| Animation p95 during parse (ms) | 5.42 | 4.63 |
| Animation p99 during parse (ms) | 5.82 | 5.72 |
| Animation missed-budget count during parse (@16.67 / @8.33) | 8 / 16 | 0 / 9 |

**Roughly a tie, Flutter marginally smoother.** Both keep the animation essentially
at frame rate while the heavy parse runs off the UI thread (Frust `spawn_blocking`,
Flutter `Isolate.run`); Flutter has the tighter tail (worst 12.67ms vs Frust
53.61ms). Release barely moved Frust here vs profile (4.61 vs 4.68 p50).

### S5 — Image pipeline (decode-and-display while scrolling)

Frust re-run post-fix `ade0d87` (release), health-gated: `layout_us > 0` on
5565/5568 frames — a valid decode-under-scroll (the earlier profile pass measured a
frozen window and was discarded).

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | 9.85 | 12.09 | 13.57 | 51.47 | 8 | 4703 |
| Flutter (profile) | 6.22 | 14.06 | 16.15 | 21.10 | 49 | 2857 |

**Split / roughly even.** Flutter has the lower median (6.2ms vs 9.9ms); Frust has
the tighter upper percentiles (p95 12.1ms vs 14.1ms, p99 13.6ms vs 16.2ms) and
fewer dropped-60Hz frames (8 vs 49). Flutter's worst is lower (21.1ms vs 51.5ms).
No clear winner — Flutter smoother at the median, Frust steadier at p95.

### S6 — Text shaping stress (multilingual relayout on width animation)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | 10.66 | 14.52 | 16.65 | 54.51 | 57 | 4439 |
| Flutter (profile) | 8.27 | 9.75 | 10.27 | 12.71 | 0 | 5093 |

**Flutter wins S6.** Flutter's text pipeline holds 8.3ms median / 9.8ms p95 with
zero dropped-60Hz frames and a 12.7ms worst; Frust/Parley medians 10.7ms with a
wider tail (p95 14.5ms, worst 54.5ms) and 57 dropped-60Hz frames. Multilingual
relayout on a width animation favors Flutter here. (Release ≈ profile: 10.66ms p50
both — text shaping is not LTO-sensitive.)

### S7 — Cold start + idle

Idle window was 30s (short-form run duration), not the protocol's 60s (deviation).
Idle memory is PSS mean over the kept runs' post-run `dumpsys meminfo` snapshots.
Frust = release.

| Metric | Frust (release) | Flutter |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of 3) | **~215 ms** | ~417 ms |
| Framework-reported first-frame span | ~147 ms (`first_frame_presented`) | 84 ms (`first_frame_ms`) |
| Idle CPU, 30s (avg %) | ~0% (below `dumpsys cpuinfo` listing threshold) | ~0% (below threshold) |
| Idle memory (PSS, mean MB) | **~98.1 MB** | ~125.7 MB |

**Split result.** On the *external, identical-tool* measurement (`am start -W`),
Frust cold-starts ~2× faster (~215ms vs ~417ms) and idles at ~22% less memory
(~98.1 vs ~125.7 MB PSS — Flutter's PSS also varied run-to-run: 149/149/88 MB).
But each framework's *own* first-frame span reports Flutter's engine reaching its
first frame sooner (84ms vs Frust ~147ms) — Frust's heavier number is GPU/vello
device+renderer init (`adapter_ready`→`renderer_ready`). Frust's frame-gate holds
both apps at ~0% idle CPU (neither appears in `dumpsys cpuinfo` when idle). Release
vs profile was within noise on both cold start (~215 vs ~198ms) and idle PSS
(~98 vs ~97 MB).

### S8 — Plugin-call overhead (shared preferences)

> **REVALIDATED 2026-07-21** under the error-accounting fix (commit `3a02cd6`
> — every write's `Result` checked, every read verified against its
> deterministic expected value, a per-op `err=0|1` field and a summary
> `s8-errors` marker line added) **and** the read-timing-symmetry fix
> (commit `bfd6fd4` — verification moved outside the timed window on both
> apps, so a read's reported `us` is the raw backend call only, matching the
> write side's always-boundary-crossing timing). Both apps rebuilt fresh from
> HEAD (`bfd6fd4`) — Frust: `frust build apk --release` (huddle keystore,
> `FRUST_TRACE`/`FRUST_TRACE_RAW` exported at build time + a `perf.rs`
> mtime-only touch to force the recompile past the release `--define` gap,
> confirmed emitting `err=0|1` fields on-device before scoring); Flutter:
> `flutter build apk --profile`. 5 runs × 30s each app, first 2 discarded
> (kept runs 3–5, 199 keys/type × 3 runs = 597 samples/type below). **Zero
> errors observed**: `grep -c s8-errors` across every kept (and discarded) raw
> log, both apps, is 0 — no write ever returned `Err`, no read was
> unexpectedly absent, no read's value mismatched its expected value. This
> directly answers the post-publication integrity concern below: the
> then-current code that produced the original numbers discarded write
> `Result`s and never verified reads, so a swallowed failure could not have
> been ruled out; it now can be, and none occurred.

Per-op median latency over kept runs (µs/call), Frust = release, computed from each
app's `*-perf plugin op=… us=… err=…` lines. Frust value types `bool`/`i64`/`f64`/
`String`/`Vec<String>` map to Flutter `bool`/`int`/`double`/`String`/`List<String>`.

| Op | Frust (µs/call, release) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | **106** | 290 | n/a |
| write i64 | **102** | 288 | n/a |
| write f64 | **69** | 309 | n/a |
| write String | **67** | 288 | n/a |
| write Vec\<String\> | **69** | 300 | n/a |
| read (unique key, forces channel) bool | **18** | 4372† | ~0 |
| read (unique key, forces channel) i64 | **18** | 4372† | ~0 |
| read (unique key, forces channel) f64 | **18** | 4372† | ~0 |
| read (unique key, forces channel) String | **18** | 4372† | ~0 |
| read (unique key, forces channel) Vec\<String\> | **18** | 4372† | ~0 |

**Frust wins S8 decisively** — the clearest and most robust Rust-advantage result,
and it holds up unchanged under the error-accounting + timing-symmetry fixes.
Writes (which always cross the boundary on both sides): Frust ~67–106µs vs Flutter
~288–309µs — a **~2.7–4.6× advantage** for direct in-process FFI (`jni`, zero
codec) over Flutter's `StandardMethodCodec` MethodChannel round-trip (essentially
the same margin as the original ~2.6–4.6×; per-type medians moved by low
single-digit µs run-to-run, well within measurement noise — see the "did timing
move" note below). †Flutter's only channel-crossing *read* is the package's
`reload()` path, which re-reads the **entire** store in one hop (median 4372µs
this pass vs 4644µs originally, both within this op's run-to-run variance) — not
a per-type single read, so the same figure is shown for every read row; Frust's
read is a real per-key backend call, still ~18µs (no Dart-style cache). Flutter's
cached read is a Dart-memory map lookup (~0µs) — genuinely fast, but not a
boundary crossing.

**Did the corrected (symmetric, verification-outside-window) timing move Frust's
read numbers?** No — Frust's read median is 18µs both before and after the
`bfd6fd4` fix (min/max also unchanged, 14–17µs to 62µs tail either way). This is
the expected result: `bfd6fd4`'s whole point was to make sure verification cost
(a cheap enum-match + scalar/string/`Vec` equality check) never entered the timed
window in the first place, on both apps — so a correctly-implemented fix should
reproduce the pre-verification (original) numbers almost exactly, not shift them.
Flutter's read (`reload()`) similarly held (4644→4372µs, within run-to-run noise
for this op — see its own p50/p95 spread above).

| Burst-during-animation variant | Frust | Flutter |
|---|---|---|
| Animation p95 during S8 burst (ms) | not run this pass | not run this pass |
| Animation missed-budget count during S8 burst | not run this pass | not run this pass |

The burst variant (`FRUST_BENCH_S8_BURST=1` / `--dart-define=S8_BURST=1`) was not
included in this short-form pass (deviation) — unchanged by this revalidation,
which was scoped to the base S8 quiescent path only.

### Phase-10 re-run — S1/S2/S5/S6, frust only (2026-07-21, raw format v2)

> **Annotated re-run, does not supersede the tables above for cross-app
> comparison.** Captured for Phase 10.A attribution (task 06 —
> `workflow/plans/features/frust-phase-10-smoothness/research/ATTRIBUTION.md`
> holds the full per-pass decomposition, first-frame span analysis, and the
> render-thread-split GO/NO-GO). **Frust only** (Flutter columns above are
> unchanged 2026-07-20 numbers). **Raw format v2** (`encode_us`+`present_us`
> split — PROTOCOL §7; sum the two to compare against a v1
> `encode_present_us` capture). **Full-form run count: 12 runs × 30s, first
> 2 discarded (10 kept)** — this re-run meets PROTOCOL §4 (the 2026-07-20
> series was 5×30s short-form). **Build includes the phase-10 optimizations
> P01–P05** (raw-v2 split, S6 shape cache, pointer-resample/deadline
> instrumentation, first-frame font-preload overlap, encode micro-wins), so
> deltas vs the rows above mix optimization effect with run-to-run variance.
> Build: repo `main` @ `59610e4`, `frust build apk --release` (huddle
> keystore, JBR `JAVA_HOME`, NDK `27.0.12077973`, `FRUST_TRACE=1
> FRUST_TRACE_RAW=1` exported at build time + `perf.rs` mtime-only touch —
> deviation #13's recipe, unchanged), v2 emission confirmed on-device and
> every scenario screencap-spot-checked before scoring. Raw series
> (filtered to perf/marker lines) under
> `raw/oneplus9/frust_release/<scenario>_p10v2/run-NN.log` (gitignored).

| Scenario (frust release, v2) | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | encode p50 / present p50 (ms) | active frames (10 kept runs) |
|---|---|---|---|---|---|---|---|---|
| S1 animation storm | 9.68 | 12.23 | 15.69 | 65.48 | 154 | 14,952 | 6.47 / 2.59 | 19,328 |
| S2 long-list scroll | 10.87 | 14.98 | 17.85 | 54.98 | 275 | 17,031 | 4.50 / 2.00 | 18,010 |
| S5 image pipeline | 9.62 | 11.49 | 13.23 | 52.26 | 58 | 15,138 | 5.44 / 2.83 | 18,970 |
| S6 text shaping | **5.67** | **10.86** | **12.20** | 56.27 | 26 | 5,105 | 3.73 / 1.43 | 30,467 |

- **S1/S2/S5 reproduce the 2026-07-20 series within noise** (p50 9.68 vs
  9.75, 10.87 vs 11.04, 9.62 vs 9.85) — the phase-10 changes did not move
  these scenarios' totals materially. Health gates held: S2 `layout_us>0`
  on 18,000/18,010 frames (mean 3,823µs), S5 on 18,960/18,970 — genuine
  scrolls; 0 skipped frames anywhere.
- **S6 improved dramatically under the 10.B shape cache** (P02): p50
  10.66→5.67ms, p95 14.52→10.86ms, and sustained rate rose ~64→~102 fps.
  Against Flutter's unchanged 2026-07-20 profile row (8.27/9.75ms):
  **frust now leads p50 by 31%; p95 is +11.4% — the 10.B "within 10%"
  target is narrowly missed** (was +48.9%). S6's frame time is now
  encode-bound like the other scenarios (text passes: layout p50 156µs).
- **Work-vs-wait split (the v2 headline):** Android `present_us` is a
  modest 1.4–2.8ms p50 everywhere (no iOS-style vsync-blocking artifact);
  **encode is the dominant pass** (3.7–6.5ms p50 = 41–67% of the median
  frame; 22–39% of the 16.67ms budget) → **render-thread split: GO** per
  RENDER_SPLIT_SPIKE.md's 10% encode-only rule (full table in
  ATTRIBUTION.md).
- **First frame (48 cold starts):** `first_frame_presented` median 131.5ms
  (123–162) vs ~147ms in the 2026-07-20 series; font preload (P04) fully
  overlapped GPU init on every launch (0ms join delta); the remaining
  floor is a flat ~71ms first-frame-encode span
  (`first_rebuild_done`→`first_encode_done`). Pipeline-cache hit/miss is
  not observable on Android — `SPAN_PIPELINE_CACHE_RESTORED` is wired
  desktop-only, and the flat first-encode span across all 48 launches
  shows no warm-start benefit (instrumentation follow-up recorded in
  ATTRIBUTION.md §2).

### Methodology deviations (this device)

1. **RESOLVED — Frust ran `--release` (PROTOCOL §2 satisfied).** The initial pass
   used a profile build (no keystore); this was fixed by signing with the same
   Android upload keystore `examples/huddle` uses, and the entire Frust column was
   re-run from a `frust build apk --release` APK (fat LTO, `strip`, `panic=abort`,
   commit `ade0d87`). Flutter stays in profile (its documented traceable mode).
   Net effect on Frust numbers vs the earlier profile build: negligible on the
   GPU-raster and plugin scenarios (S1/S4/S6/S7/S8 within noise), and S2/S5 are now
   *valid* independent of the build mode (see #11).
2. **Short-form run count.** 5 runs × 30s per scenario (first 2 discarded, ≥3
   kept), not PROTOCOL §4's ≥10-runs full form — a first-fill within the wall-clock
   budget. Raw series for all 5 runs (including the 2 discarded) are retained
   locally (not committed — see #12).
3. **Flutter release-mode in-app cross-check (PROTOCOL §2) not captured** — only
   the profile-mode headline series was taken.
4b. **RESOLVED — S1 PARITY-SUSPECT.** The v1→v2→v3 parity chain (see the S1
   section above) closed the workload-mismatch gap; both apps re-run under v3
   (commit `89ca8e8`) produced a **split result** (Frust wins tail/jank
   consistency, Flutter wins median/raw fps) — see the S1 section's full table
   and narrative. No numbers from the original PARITY-SUSPECT pass are used
   in the split verdict.
4c. **RESOLVED (health) — S3 SUSPECT.** Root-caused to the op sequence running
   once and legitimately emptying at `clear` (not occlusion/glitching); the
   continuous-cycling redesign (commit `73fa17f`) fixed it, and a fresh re-run
   confirms healthy, non-empty, repeatedly-cycling behavior on both apps — see
   the S3 section above. No overall winner is declared (see #4 below, now
   worse than originally recorded), but Frust's per-op numbers are usable.
4. **WORSENED — S3 per-op: Flutter capture failed entirely this pass.** The
   original short-form pass caught one `s3-create1k` frame inside its
   sub-marker window by luck; the final continuous-cycling re-run caught
   **zero** frames in any of the five per-op windows, across all three kept
   runs — confirmed structural, not incidental: every Flutter
   `bench-scenario-end s3-<op>` line is immediately preceded by its `start`
   with no `flutter-perf raw` line between them, because
   `SchedulerBinding.addTimingsCallback` delivers timing data on a later
   event-loop turn than the synchronous marker calls. Frust captured all five
   ops cleanly both times. A real fix needs the Flutter bench app's marker
   placement reworked to fire `end` from inside the async timing callback
   itself — out of this task's scope (would be a `flutter_bench` source
   change, not a benchmark-running task).
5. **S4 parse wall-time not recoverable** — the harness captures logcat `-v raw`
   (no timestamps) and neither app emits an explicit parse-duration line. Only the
   animation-during-parse frame percentiles are reported.
6. **S7 idle window was 30s, not 60s** (short-form run duration). Idle CPU could
   not be sampled — both apps fell below `dumpsys cpuinfo`'s listing threshold when
   idle (reported as ~0%). Idle memory is Android PSS (not comparable to iOS RSS).
   `am start -W` external cold start was measured post-matrix (4 launches/app,
   first discarded).
7. **S8 burst-during-animation variant not run** this pass.
8. **Thermal control was gate-based, not fixed-pause.** `device_state.sh` gated on
   `dumpsys battery` temperature (ceiling 38°C) before each block; device was
   30.4°C at every block start, so no forced cooldown/idle pause occurred beyond
   the screen being slept between blocks.
9. **Charger:** device stayed USB-connected (adb requirement); `dumpsys battery
   unplug` presented an on-battery state to the governor identically for both apps
   (reset after the run). Airplane mode was on; Wi-Fi disabled during the run.
10. **Harness portability fix (no result impact):** run.sh's `mktemp
    run-XXXXXX.log` template only expands under GNU mktemp; on this macOS host a
    PATH shim aliased `mktemp`→`gmktemp` so run.sh ran unedited. The harness script
    itself was not modified.
11. **RESOLVED — S2 + S5 re-run valid.** The original profile pass measured a
    frozen scroll window (`layout_us≈0`) because its APK predated the `ade0d87`
    scroll-relayout fix. The Frust column here is a release re-run against that fix,
    health-gated before use: S2 `layout_us>0` on 5319/5322 frames (mean 3802µs), S5
    on 5565/5568 — genuine scrolls. Frust wins S2 with valid numbers; S5 is a split.
12. **Raw logs NOT committed (privacy).** Per a user directive, full-device logcat
    captures never enter git — `benchmarks/raw/oneplus9/` is gitignored. RESULTS.md
    is the committed deliverable; the raw series (filtered to perf/marker lines)
    stays local for inspection/reproduction.
13. **Release trace-injection quirk (build-recipe note).** In a `--release` build,
    Gradle's `--define`→`environment()` mechanism did not reach the cargo-ndk
    compile, so `option_env!("FRUST_TRACE")` didn't bake and the release APK emitted
    no `frust-perf` lines. Worked around by exporting `FRUST_TRACE=1
    FRUST_TRACE_RAW=1` in the build shell (cargo-ndk inherits the env) and forcing a
    recompile. The profile build was unaffected (it auto-injects `FRUST_TRACE`).
    Worth a real fix in the bench/CLI if release-build tracing is expected to work
    from `--define` alone.
14. **Screen-lock / taint audit.** A `dumpsys battery unplug` side effect re-enabled
    the screen timeout, locking the screen between scenario switches. The runner
    already wakes + dismisses keyguard at each block start; `svc power stayon true`
    was additionally set mid-run. **Taint audit (per-run frame continuity):** all 8
    release scenarios' kept runs (3–5) show consistent, healthy frame counts with no
    screen-off signature (near-zero/gap). Only discarded run1s were low (e.g. S3
    run1=0). No re-run needed. `svc power stayon` restored to `false` at the end.
15. **S1/S3 final re-run build note.** Both apps rebuilt fresh from commit
    `73fa17f` for this pass (S1 v3 + S3 continuous cycling, superseding the
    `ade0d87`-era S1/S3 builds S2/S4/S5/S6/S7/S8 above still reflect — those six
    scenarios are unaffected by the S1/S3-only spec changes and were not
    re-run). Same recipe as the original release pass: JBR JAVA_HOME, NDK
    27.0.12077973, `FRUST_TRACE=1 FRUST_TRACE_RAW=1` exported at build time plus
    a `crates/frust-shell-common/src/perf.rs` mtime-only touch (confirmed empty
    `git diff`) to force the recompile past Gradle's `--define` release-mode gap
    (deviation #13, unchanged). Raw-line emission (`frust-perf raw`/
    `bench-scenario-start`, and `flutter-perf raw`) was directly confirmed
    on-device for both S1 and S3 before any run was scored, per this task's
    explicit gate.
16. **S1/S3 final re-run device-state note.** Airplane mode on
    (`settings put global airplane_mode_on 1`, confirmed `1` on read-back),
    Wi-Fi disabled (`svc wifi disable`, confirmed "Wi-Fi is disabled"),
    `dumpsys battery unplug` presented an on-battery state identically for both
    apps (USB stayed physically connected for adb), `svc power stayon true` set
    for the duration and restored to `false` afterward. Brightness fixed at 128
    via `device_state.sh`. Thermal: 31.8–33.7°C throughout this session (ceiling
    38°C never approached; no forced cooldown wait triggered). No screen-lock/
    dream taint observed (`dumpsys window`'s `mDreamingLockscreen=false`,
    `dumpsys power`'s `mWakefulness=Awake` checked mid-session). Same `gmktemp`
    PATH-shim workaround as before (deviation #10, unchanged) — `run.sh` itself
    still not modified.
17. **S8 final re-run build/device-state note (2026-07-21, commit `bfd6fd4`).**
    Both apps rebuilt fresh: Frust `frust build apk --release` (huddle keystore,
    JBR `JAVA_HOME`, NDK `27.0.12077973`, `FRUST_TRACE=1 FRUST_TRACE_RAW=1`
    exported at build time + a `crates/frust-shell-common/src/perf.rs`
    mtime-only touch — confirmed empty `git diff` — to force the recompile past
    the release `--define` gap (deviation #13, unchanged)); Flutter
    `flutter build apk --profile`. Confirmed `err=0|1` fields present on-device
    for both apps before scoring. Package id `it.f0x.flutter_bench` (not the
    harness's `it.f0x.flutterbench` default — passed via `--pkg`, same note as
    the original pass). Device state: airplane mode on, Wi-Fi disabled,
    `dumpsys battery unplug`, brightness 128, `svc power stayon true` for the
    duration — all restored afterward (confirmed via read-back). Thermal
    ~29.5–30°C throughout (well under the 38°C ceiling). `run.sh`'s `mktemp`
    bug (deviation #10) no longer reproduces — a prior commit
    (`0684e56`, "h4-harness-mktemp") already fixed the run-log naming to a
    deterministic `run-NN.log` scheme, so no `gmktemp` PATH shim was needed
    this pass; noted here since it resolves that long-standing workaround note
    for any future S8 (or other-scenario) re-run on this macOS host.
18. **Phase-10 re-run (2026-07-21) build/device-state note.** Frust-only
    S1/S2/S5/S6 re-run (see its own section above): full-form 12×30s runs
    (first PROTOCOL §4-compliant pass on this device — deviation #2 does not
    apply to it); airplane mode on (`settings put`, read-back 1; the
    AIRPLANE_MODE broadcast is shell-denied on Android 15, same as the
    S1/S3 final re-run), Wi-Fi disabled, `dumpsys battery unplug`
    re-applied before each scenario block (run.sh's exit cleanup resets the
    spoof between blocks), brightness 128, `svc power stayon true` during
    blocks; thermal 27.7–36.2°C, 38°C ceiling never reached; all state
    restored + read-back confirmed after the session. `run.sh` unmodified
    (deterministic `run-NN.log` naming from the H4 mktemp fix — no PATH
    shim needed, matching deviation #17's finding). Flutter was NOT re-run
    (task scope: frust attribution only), so S6's cross-app delta compares
    a 10-kept-run frust series against the 3-kept-run 2026-07-20 Flutter
    series. S6's per-run frame counts were bimodal (2,374–3,578 per 30s —
    ~80 vs ~120 fps regimes, a display-mode/DVFS effect at the panel's two
    advertised modes); combined percentiles span both regimes.

---

## Device: iPhone SE — iOS

**Status:** run 2026-07-21 (short-form: 5 runs × 30s, first 2 discarded — see
run-count deviation). **Frust = RELEASE build; Flutter = profile (per protocol
§2).** All eight scenarios captured on the physical iPhone SE via `xcrun
devicectl` (iOS 17+ automation, `benchmarks/harness/run.sh --platform ios`).
**S8 was additionally re-run 2026-07-21 from commit `bfd6fd4`** (the
error-accounting + read-timing-symmetry fixes — see S8's own banner below);
S1–S7 below are unaffected and reflect the original same-day pass.
Raw series under `raw/iphone_se/<app>/<scenario>/run-0N.log` (S8's final
re-run additionally under `.../s8_reval/run-0N.log`; gitignored — device
console/container captures never enter git, filtered to perf/marker lines
only).

**Read the two iOS instrumentation deviations (below) before the frame
tables** — they materially change how S1/S2/S4/S5/S6 per-frame numbers must be
read. In short: the only two *cleanly cross-comparable* results this pass are
**S8** (plugin latency — direct µs timing, Frust wins decisively) and **S7**
(startup spans). For the render/animation scenarios both frameworks sustain
~60 fps with large headroom on this A13 device and **no framework drops a real
frame**; a per-frame render *winner* is not declared because the two available
frame-time metrics measure different things on iOS (Frust's includes
present-to-vsync idle; Flutter's spec'd `totalSpan` is invalid on iOS).

- Chipset: Apple A13 Bionic (iPhone SE 2nd gen, `iPhone12,8`)
- OS version: iOS 26.5.2
- Fixed brightness: **uncontrolled** (iOS has no brightness CLI — deviation)
- Refresh-rate mode: 60Hz (iPhone SE has no ProMotion panel — the 8.33ms
  budget column is N/A for every scenario on this device; per protocol §5)
- Flutter version: 3.44.2 stable — `flutter build ios --profile
  --dart-define=SCENARIO=<sN>` (one profile build per scenario; the team
  `87MFQ5L648` was already set in the committed Xcode project, so signing
  needed no file edit). Info.plist already carries
  `CADisableMinimumFrameDurationOnPhone` (fairness gate §6, satisfied).
- Frust build: **RELEASE** (`frust build ios --release`, signed
  `FRUST_IOS_TEAM=87MFQ5L648`), one build serving every scenario (repo near
  commit `36d5451`). `FRUST_TRACE=1 FRUST_TRACE_RAW=1` baked at compile time
  via `frust build ios --define` (the iOS run-script exports `FRUST_DEFINES`
  before `cargo build`, so `option_env!` reads them); confirmed emitting
  `frust-perf raw`/`bench-scenario-start` on-device before scoring.
- Thermal-cooldown method: **fixed 15s inter-scenario wait** (iOS has no
  battery/thermal-sensor CLI — no gate possible; uncontrolled, deviation).
- Charger/airplane/brightness: **all uncontrolled** (no iOS CLI for any of
  them — deviation). Screen kept awake by the user for the run.

**Scenario-selection mechanism (per app, documented in `run.sh`'s header and
PROTOCOL §8):**
- **Frust — runtime env, one build.** `devicectl device process launch
  -e '{"FRUST_BENCH_SCENARIO":"<sN>",...}'` delivers the scenario env var to
  the process, which `resolve_initial()` reads — a clean **cold** selection
  (no s1→scenario warm-switch flash, important for S7), and the app's raw
  `frust-perf`/`bench-scenario` stdout is captured live off `--console`.
- **Flutter — compile-time define, one build per scenario.** Dart's
  `String.fromEnvironment('SCENARIO')` is compile-time, so each scenario is a
  separate `--dart-define=SCENARIO=<sN>` profile build (8 builds). Flutter's
  `print` routes to os_log, which neither `devicectl --console` nor the
  (empty on iOS 26) legacy syslog relay surfaces, so the bench app writes its
  trace to a file in its `tmp/` container that the harness pulls with
  `devicectl device copy from --domain-type appDataContainer` after each run.

### S1 — Animation storm

Frame total: **Frust = `total_us`** (pass-sum, *includes* CADisplayLink
present-to-vsync wait — see deviation 2); **Flutter = `build_us+raster_us`**
(work-sum, since `totalSpan` is invalid on iOS — deviation 1).

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | 5.69 | 16.66† | 16.82† | 42.23 | 259† | N/A (60Hz panel) |
| Flutter (profile) | 5.16 | 5.52 | 5.67 | 19.78 | 1 | N/A (60Hz panel) |

†Frust's p95/p99/missed here are **present-to-vsync idle**, not render cost:
across the 3 kept runs, 2 were work-timed (~5.5ms) and 1 vsync-locked
(~16.5ms, present blocking on the 60Hz cadence). Frust's *stable* render work
(rebuild+layout+paint) is **~1.6ms p50** every run; its GPU-encode adds ~4ms →
~5.5ms real frame work, matching the work-timed runs and comparable to
Flutter's ~5.2ms. **Both sustain ~60 fps (Frust ~59, Flutter ~65 reported);
neither drops a real frame.** No per-frame winner declared (metric asymmetry).

### S2 — Long-list scroll (10k rows)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | 5.28 | 6.37 | 7.82 | 38.11 | 4 | N/A (60Hz panel) |
| Flutter (profile) | 4.86 | 5.42 | 6.05 | 9.94 | 0 | N/A (60Hz panel) |

Kept Frust runs were work-timed here (not vsync-locked), so these are true
render costs: ~5.3ms Frust vs ~4.9ms Flutter — **essentially even**, both far
inside a 60Hz frame. Frust renders continuously (~1779 frames/run) vs
Flutter's event-driven scroll (~648 frames/run).

### S3 — Table ops (js-framework-benchmark subset)

Continuous cycling confirmed on both apps (kept-run `s3-create1k` reopen
counts: Frust 18, Flutter 21). **Per-op reconcile timing** (`s3-*`
sub-markers, aggregated across cycles in the 3 kept runs):

| Op | Frust p50/p95/p99 (ms, release) | n frames | Flutter |
|---|---|---|---|
| create 1k | 10.49 / 11.88 / 13.61 | 51 | **not captured** |
| create 10k | 8.62 / 9.30 / 11.71 | 54 | **not captured** |
| update every 10th of 10k | 6.92 / 7.55 / 7.66 | 54 | **not captured** |
| swap | 6.57 / 7.23 / 7.48 | 54 | **not captured** |
| clear | 5.34 / 6.12 / 6.32 | 51 | **not captured** |

**Flutter per-op capture landed zero frames in every sub-marker window**, all
three kept runs — the identical structural gap seen on the OnePlus 9 (deviation
4 there): `SchedulerBinding.addTimingsCallback` delivers each frame's timing on
a later event-loop turn than the synchronous `bench-scenario-end s3-<op>` call,
so the op's frames always land *after* its window closes. Frust captured all
five ops cleanly. Not a Frust advantage in workload, a Flutter-side marker-vs-
callback timing gap (a `flutter_bench` fix, out of scope). Whole-series (marker
`s3`): Frust p50 5.32 / p95 6.99ms (5317 frames, continuous ~59 fps); Flutter
p50 4.90 / p95 12.31ms (309 frames, event-driven — paints only at op
transitions) — not apples-to-apples (continuous vs event-driven paint).

### S4 — Heavy-work responsiveness (~50MB JSON parse + concurrent animation)

| Metric | Frust (release) | Flutter |
|---|---|---|
| Total wall time (parse complete) | not captured (see deviation 5) | not captured |
| Animation p50 during parse (ms) | 16.34† | 1.33 |
| Animation p95 during parse (ms) | 16.64† | 1.60 |
| Animation p99 during parse (ms) | 16.83† | 1.73 |
| Animation missed-budget count during parse (@16.67 / @8.33) | 211† / N/A | 0 / N/A |

†Frust's kept S4 runs were mostly vsync-locked (present-wait), so 16.3ms is
frame **wall-time at the 60fps cadence**, not render cost — Frust's render work
stayed ~1.1ms CPU + ~4ms GPU here too. **Both apps held the animation at ~60fps
throughout the parse** (Frust `spawn_blocking`, Flutter `Isolate.run` — the
heavy parse ran off the UI thread on both); neither dropped a real frame.
Roughly even. Parse wall-time is not recoverable (deviation 5).

### S5 — Image pipeline (decode-and-display while scrolling)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms |
|---|---|---|---|---|---|
| Frust (release) | 5.53 | 16.65† | 16.90† | 38.64 | 247† |
| Flutter (profile) | 2.68 | 3.59 | 3.90 | 5.86 | 0 |

†Same present-wait artifact (1 of 3 kept Frust runs vsync-locked). Frust's
work-timed runs read ~5.3ms; Flutter's work-sum ~2.7ms. Both sustain ~60fps,
no real drops. No clean per-frame winner (metric asymmetry).

### S6 — Text shaping stress (multilingual relayout on width animation)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms |
|---|---|---|---|---|---|
| Frust (release) | 5.55 | 6.90 | 7.60 | 39.48 | 3 |
| Flutter (profile) | 4.22 | 4.90 | 5.15 | 16.42 | 1 |

Kept Frust runs work-timed (not vsync-locked), so true render costs:
Frust ~5.6ms vs Flutter ~4.2ms — **both comfortably inside a 60Hz frame**,
roughly even (Flutter marginally lower, as on the OnePlus 9's S6). Parley
multilingual relayout keeps pace with Flutter's text pipeline on this device.

### S7 — Cold start + idle

| Metric | Frust (release) | Flutter |
|---|---|---|
| External cold start (process spawn → first frame) | N/A (deviation 6) | N/A (deviation 6) |
| Framework-reported first-frame span | ~188 ms (`first_frame_presented`) | **~7 ms** (`first_frame_ms`) |
| Idle CPU, 30s (avg %) | N/A (deviation 6) | N/A (deviation 6) |
| Idle memory (RSS, MB) | N/A (deviation 6) | N/A (deviation 6) |

**Split (Flutter wins the one captured metric).** Flutter's framework
first-frame (`main()`→first frame, ~7ms) is far quicker than Frust's
app-entry→first-frame (~188ms median of 3: 183/188/192ms). Frust's time is
dominated by `first_rebuild_done` (~170ms — first build+layout of the scene);
GPU init (`adapter_ready`→`renderer_ready`) is only ~17ms. This mirrors the
OnePlus 9 pattern (Flutter's engine reaches its own first frame sooner). The
*external* cold-start (which favored Frust ~2× on Android via `am start -W`)
has **no devicectl/CLI equivalent on iOS** — not measurable this pass
(deviation 6), so the cross-tool comparison that decided S7 on Android is
absent here. Idle CPU/memory (RSS) likewise have no cheap devicectl readout.

### S8 — Plugin-call overhead (shared preferences)

> **REVALIDATED 2026-07-21** under the same error-accounting (`3a02cd6`) +
> read-timing-symmetry (`bfd6fd4`) fixes as the OnePlus 9 re-run above — see
> that section's banner for the full mechanism description. Both apps rebuilt
> fresh from HEAD (`bfd6fd4`): Frust `frust build ios --release`
> (`FRUST_IOS_TEAM=87MFQ5L648`, `FRUST_TRACE`/`FRUST_TRACE_RAW` baked via
> `--define`, confirmed emitting `err=0|1` fields via a `devicectl --console`
> smoke launch before scoring); Flutter `flutter build ios --profile
> --dart-define=SCENARIO=s8`. Same 5×30s/first-2-discarded short form, same
> `devicectl` env-launch (Frust) / on-device trace-file-pull (Flutter) capture
> mechanism as the original iPhone SE pass. **Zero errors observed** — no
> `s8-errors` marker line in any kept (or discarded) raw log on either app.

Per-op median latency over kept runs (µs/call, first call per type excluded as
warmup). Frust `bool`/`i64`/`f64`/`String`/`Vec<String>` ↔ Flutter
`bool`/`int`/`double`/`String`/`List<String>`.

| Op | Frust (µs/call, release) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | **3** | 106 | n/a |
| write i64 | **3** | 80 | n/a |
| write f64 | **3** | 54 | n/a |
| write String | **3** | 56 | n/a |
| write Vec\<String\> | **5** | 57 | n/a |
| read (unique key, forces channel) bool | **1** | 2674† | ~0 |
| read (unique key, forces channel) i64 | **1** | 2674† | ~0 |
| read (unique key, forces channel) f64 | **1** | 2674† | ~0 |
| read (unique key, forces channel) String | **1** | 2674† | ~0 |
| read (unique key, forces channel) Vec\<String\> | **1** | 2674† | ~0 |

**Frust wins S8 decisively** — the clearest cross-comparable Rust-advantage
result on this device, matching the OnePlus 9, and it holds up unchanged under
the fixes. Writes (always cross the boundary on both sides): Frust ~3–5µs vs
Flutter ~54–106µs — a **~14–29× advantage** for direct in-process FFI
(`objc2`/`NSUserDefaults`, zero codec) over Flutter's `StandardMethodCodec`
MethodChannel round-trip (essentially the same margin as the original
~13–30×; the write medians moved a few µs run-to-run on the Flutter side,
noise typical of this device's smaller sample). †Flutter's only
channel-crossing *read* is the package's `reload()` path (re-reads the entire
store in one hop) — its median landed at **exactly 2674µs again**, unchanged
to the µs, not a per-type read, shown for every read row; Frust's read is a
real per-key backend call, still **~1µs**. Flutter cached reads are
Dart-memory map lookups (~0µs — fast, but not a boundary crossing).

**Did the corrected timing move Frust's read numbers?** No — Frust's read
median stayed at 1µs before and after `bfd6fd4`, same as the OnePlus 9 finding:
moving read verification outside the timed window (on both apps) reproduces
the original numbers rather than shifting them, because the verification cost
it removed was never large enough to register at this device's already-tiny
per-key latency.

| Burst-during-animation variant | Frust | Flutter |
|---|---|---|
| Animation p95 during S8 burst (ms) | not run this pass | not run this pass |
| Animation missed-budget count during S8 burst | not run this pass | not run this pass |

The burst variant was not included in this short-form pass (deviation 7) —
unchanged by this revalidation, scoped to the base S8 quiescent path only.

### Methodology deviations (this device)

1. **Flutter frame total = `build_us+raster_us`, not `totalSpan`.** On this
   iOS device `FrameTiming.totalSpan` is frequently **negative** under load
   (S4/S5 p50 came out < 0 — a documented iOS pipeline-overlap quirk), so it is
   unusable as the protocol §5 "total_us" column. The work-sum
   `buildDuration+rasterDuration` (always non-negative, and the direct analog
   of Frust's pass-sum) is used for every Flutter iOS scenario instead. The raw
   `total_us=<totalSpan>` field is still in the committed-format raw lines for
   inspection.
2. **Frust frame `total_us` includes present-to-vsync idle on iOS.** Frust's
   `encode_present_us` on iOS folds in the CADisplayLink present() blocking on
   the 60Hz vsync, so `total_us` is **bimodal per run** (~5.5ms work-timed vs
   ~16.5ms cadence-locked) and its p95/p99/missed-budget entries marked † above
   reflect *idle vsync wait, not render cost*. Frust's stable render **work**
   (rebuild+layout+paint) is ~1.1–1.6ms p50 on every scenario/run; it never
   drops a real frame (sustained ~59 fps). Consequence: **no per-frame render
   winner is declared for S1/S2/S4/S5/S6** — the two frameworks' available
   frame-time metrics measure different things on iOS. Clean cross-framework
   signal this pass comes from S8 (plugin µs latency) and S7 (startup spans).
3. **Frust ran `--release` (PROTOCOL §2 satisfied)** — signed device build,
   `FRUST_IOS_TEAM=87MFQ5L648`, one build for all scenarios. Flutter stays in
   profile (its documented traceable mode).
4. **Short-form run count.** 5 runs × 30s per scenario (first 2 discarded, 3
   kept), not §4's ≥10 — first iOS fill within the wall-clock budget. All 5
   runs' raw series retained locally (gitignored).
5. **S4 parse wall-time not recoverable** — the filtered per-line capture
   carries no timestamps and neither app emits an explicit parse-duration line
   (same as the OnePlus 9). Only animation-during-parse percentiles reported.
6. **iOS has no CLI for external cold-start, idle CPU, or memory (RSS).**
   `devicectl` exposes no `am start -W` equivalent, no `cpuinfo`, and no cheap
   RSS readout, so S7's external cold-start (the metric that favored Frust ~2×
   on Android), idle CPU, and idle memory are **not measurable** this pass —
   only each framework's own first-frame span was captured. Per protocol §S7,
   iOS RSS would not be cross-comparable with Android PSS anyway.
7. **S8 burst-during-animation variant not run** this pass.
8. **Environmental controls uncontrolled (no iOS CLI).** Brightness, charger
   state, airplane mode, and thermal are all uncontrollable via CLI on iOS;
   cooldown was a fixed 15s inter-scenario wait, not a sensor-gated pause. The
   ~47-minute sustained-render matrix did heat the device, but because Frust's
   render *work* stayed flat (~1.2ms CPU) across runs, the observed ~16.5ms
   runs are present-wait, **not** thermal throttling of render work — verified
   by the stable per-run CPU-pass medians.
9. **Scenario selection differs by app (both automatic, no manual taps):**
   Frust via `devicectl -e` env (cold, one build); Flutter via
   `--dart-define=SCENARIO` (one profile build per scenario). See the
   mechanism note above and `run.sh`'s header.
10. **Capture transport differs by app:** Frust via `devicectl --console`
    (stdout, live); Flutter via an on-device `tmp/` trace file pulled with
    `devicectl device copy from` (Flutter `print` reaches neither the console
    nor the empty-on-iOS-26 syslog relay). Both are sliced by the identical
    `stats.py` from identical `*-perf raw`/`bench-scenario` lines, so the
    transport does not affect any computed number. Two Flutter runs (S1 run2,
    S6 run1) returned an empty file (a launch/pull hiccup) — both were discarded
    warm-up runs (run 1/2), so no kept run was affected.
11. **S8 final re-run build/device-state note (2026-07-21, commit `bfd6fd4`).**
    Same recipe as the original iPhone SE pass: Frust `frust build ios --release`
    (`FRUST_IOS_TEAM=87MFQ5L648`), Flutter `flutter build ios --profile
    --dart-define=SCENARIO=s8`; both confirmed emitting `err=0|1` fields via a
    manual `devicectl --console`/container-pull smoke launch before any scored
    run. Environmental controls (brightness/charger/airplane/thermal) remain
    uncontrolled per deviation 8, unchanged. Both bench apps force-quit and
    confirmed not running (`devicectl device info processes`) after the pass;
    no other device state to restore on iOS.

---

## Device: OnePlus 15 (Adreno 840) — headline Android

**Status:** pending device availability — not yet run. This column is the
plan's headline device (proven S1 Impeller-vs-vello win on Adreno 840,
now to be formalized across the full matrix); added here whenever the
device is next at desk. Copy the OnePlus 9 section's table shapes above
when filling this in.

---

## Cross-device narrative

Two devices now have a full matrix — the **OnePlus 9** (Adreno 660, mid-tier
Android) and the **iPhone SE 2nd gen** (Apple A13, iOS); the headline Adreno 840
(OnePlus 15) remains pending. This is a two-tier snapshot across two OSes, still
not the headline-GPU story.

### iPhone SE (Apple A13, iOS 26) — 2026-07-21

The A13 has so much headroom that **neither framework drops a real frame on any
of the eight scenarios** (Frust ~59 fps, Flutter ~65 fps reported, sustained).
Two iOS-specific instrumentation facts dominate how this device's numbers read
(both fully documented in the iPhone SE deviations): Flutter's
`FrameTiming.totalSpan` is invalid on iOS (negative under load) so its frame
total is reported as the `build+raster` work-sum; and Frust's `total_us` folds
in the CADisplayLink present-to-vsync idle wait, making it a bimodal wall-time
(~5.5ms work-timed vs ~16.5ms cadence-locked) rather than a pure render cost —
Frust's *render work* is a flat ~1.2ms CPU + ~4ms GPU every run. Because the two
available per-frame metrics measure different things on iOS, **no per-frame
render winner is declared for S1/S2/S4/S5/S6** — both simply have ample 60Hz
headroom. The two cleanly cross-comparable results:
- **Frust wins S8 decisively** (plugin-call overhead; revalidated 2026-07-21
  under the error-accounting + timing-symmetry fixes, zero errors, numbers
  unchanged from the original pass to within noise): writes ~3–5µs vs
  Flutter's ~54–106µs (**~14–29×**), and a forced-channel read of ~1µs vs
  Flutter's whole-store `reload()` at ~2674µs — the same robust Rust-advantage
  seen on the OnePlus 9, via `objc2`/`NSUserDefaults` direct FFI vs the
  MethodChannel `StandardMethodCodec` round-trip.
- **Flutter wins S7's one captured metric** (framework first-frame ~7ms vs
  Frust's ~188ms — Frust's is dominated by the first scene build+layout, GPU
  init only ~17ms), mirroring Android. iOS's lack of an `am start -W`
  equivalent means the *external* cold-start that favored Frust ~2× on Android
  could not be measured here.
- **S3** per-op: Frust captured all five ops cleanly (~5.3–10.5ms); Flutter's
  per-op sub-marker capture again landed zero frames (the same
  `addTimingsCallback` async-delivery gap as the OnePlus 9, not a workload
  issue). **S2/S4/S5/S6** are roughly even on render work, both inside a 60Hz
  frame with room to spare.

### OnePlus 9 (Adreno 660, mid-tier Android)

On this **mid-tier** device, in short form (5×30s), **Frust = release** (huddle
keystore), **Flutter = profile**:

- **Frust wins:** **S2** (long-list scroll — valid post-fix release re-run: ~11.0ms
  vs ~15.3ms median, 104 vs 666 dropped-60Hz frames); **S8** (plugin-call overhead
  — revalidated 2026-07-21 under the error-accounting + timing-symmetry fixes,
  zero errors, writes ~2.7–4.6× faster, channel-crossing reads 18µs vs 4372µs —
  the clearest, most robust Rust-advantage result, unchanged from the original
  pass to within noise); **S7 external cold start** (`am start -W`
  ~215ms vs ~417ms) and **idle memory** (~98MB vs ~126MB PSS).
- **Flutter wins:** **S6** (text shaping — ~8.3ms vs ~10.7ms median); **S7
  framework-reported first-frame** (84ms vs ~147ms — Frust's GPU/renderer init is
  heavier).
- **Roughly even / split:** **S4** (heavy-work — both hold frame rate off-thread,
  Flutter marginally smoother tail); **S5** (image pipeline, valid post-fix — Flutter
  lower median 6.2 vs 9.9ms, Frust tighter p95/p99 and fewer drops); **S1**
  (animation storm, now under the fair v3 settled spec — medians within noise
  of each other, Frust far fewer 60Hz-budget drops (52 vs 459, 0.9% vs 5.1% of
  frames rendered) and tighter p95/p99, Flutter a higher sustained frame rate
  and marginally lower median — see S1 section for the full split rationale).
- **No overall winner (per-op capture gap, one side only):** **S3** (table ops,
  now continuously cycling and health-confirmed on both apps — Frust's per-op
  reconcile costs are cleanly measured (~12.5–24ms depending on op, all but a
  create-1k tail outlier inside a 60Hz budget), but Flutter's per-op sub-marker
  capture landed zero frames this pass (an `addTimingsCallback` async-delivery
  gap, not a Frust deficiency), so no head-to-head per-op number exists; the
  whole-series totals aren't comparable either, since Frust paints continuously
  through the settle gaps while Flutter only paints at op transitions).

The honest headline for this device: **Frust's demonstrated advantages are at the
plugin boundary (S8), in long-list scroll (S2), and in cold-start/idle-footprint
(S7); Flutter's mature text pipeline wins S6 and reaches its own first frame
sooner.** S1 (animation storm) is now a fair, workload-matched comparison under
the v3 settled spec, and reads as a genuine split rather than an open question:
Frust is the smoother/less-janky renderer at this workload, Flutter the
higher-throughput one — the plan's marquee S1 vello-vs-Impeller claim remains
Adreno-840-specific and unconfirmed here either way, to be re-measured on the
OnePlus 15. S3 (table ops) is now health-confirmed correct on both apps
(continuous cycling, no glitching) but still has no cross-app per-op number
because of a Flutter-side instrumentation gap, not a workload problem — a real
fix (Flutter marker placement) is a follow-up, not a re-run. Moving Frust from
profile to a true `--release` build changed the per-frame and plugin numbers
only within noise (S1/S4/S6/S7/S8) — the wins/losses are structural, not a
build-mode artifact.

---

## Post-publication integrity note — RESOLVED 2026-07-21

**Resolution.** The S8 sections above (both devices) are now the revalidated
series, captured under the fixed binaries (error-accounting `3a02cd6` +
read-timing-symmetry `bfd6fd4`), superseding the pre-fix numbers this note
originally flagged. Every write's `Result` is now checked and every read
verified against its deterministic expected value, with a per-op `err=0|1`
field and a summary `s8-errors` marker exposing any failure; **zero errors
were observed across both devices, both apps, all five value types, both
write and read ops** (no `s8-errors` marker in any kept — or discarded — raw
log). The read-timing-window asymmetry (Rust previously timed read+verify vs
Flutter's read-only) is also corrected: verification now runs after the timed
window closes on both apps, so the timed `us` is the raw backend call only on
both sides. The corrected numbers track the original pre-fix numbers closely
(Frust reads unchanged at 18µs/Android and 1µs/iOS; writes moved by low
single-digit-to-low-tens of µs, consistent with ordinary run-to-run variance —
see each device's S8 section for the full before/after breakdown), so the
original headline direction and margins were not an artifact of the
swallowed-failure/timing-asymmetry gaps this note flagged.

## App size (release)

A one-time size snapshot, not a per-device frame benchmark — measured on the
build host via the new `benchmarks/harness/app_size.sh` (degrades to a
printed "not built" note per artifact rather than failing; reruns the same
`unzip -l`/`du -sk` measurements below on demand). Both apps' Android release
APK (universal, all three ABIs, plus an arm64-v8a-only split for a same-ABI
comparison) and iOS release `.app` bundle.

### Android release APK

| App | Universal APK (arm64+armv7+x86_64) | arm64-v8a split |
|---|---|---|
| Frust | **21.16 MB** (22,188,018 bytes) | **7.49 MB** (7,858,991 bytes)† |
| Flutter | 43.51 MB (45,628,582 bytes) | 15.48 MB (16,236,261 bytes) |

†Measured directly off the just-built artifact (`app-arm64-v8a-release.apk`,
7,858,991 bytes) but not left on disk for `app_size.sh` to re-report:
Frust's Gradle project writes every release-variant APK to the same
`android/app/build/outputs/apk/release/` directory regardless of
`--split-per-abi`, so rebuilding the universal APK afterward (to restore
the run-matrix's default artifact) overwrote the split one in place.
Flutter's `build/app/outputs/flutter-apk/` directory keeps every variant
side by side, so its arm64 split stays on disk and is directly
re-measurable by `app_size.sh`.

**Build commands:**
- Frust universal: `(cd benchmarks/frust_bench && frust build apk --release)`
  — already built by the run-matrix campaign; rebuilding it this pass
  reproduced the identical 22,188,018-byte artifact (deterministic release
  build).
- Frust arm64 split: `(cd benchmarks/frust_bench && frust build apk --release
  --split-per-abi --target-platform android-arm64)` — same huddle upload
  keystore, JBR `JAVA_HOME`, NDK `27.0.12077973` as the OnePlus 9 release
  recipe above (`docs/DEVELOPMENT.md`'s Prerequisites).
- Flutter universal: `(cd benchmarks/flutter_bench && flutter build apk
  --release)` — debug-signed (Flutter's default with no `key.properties`
  present; fine for a size comparison, doesn't affect artifact size).
- Flutter arm64 split: `(cd benchmarks/flutter_bench && flutter build apk
  --release --split-per-abi)`.

**Per-ABI `.so`/dex breakdown** (`unzip -l` summed by path — the
`scripts/size-report.sh` technique; these are uncompressed listing sizes, so
a row sum can exceed the compressed APK total):

| Component | Frust (universal APK) | Flutter (arm64 split APK) |
|---|---|---|
| `lib/arm64-v8a/*.so` (engine + app) | 7.39 MB (7,748,168 bytes) | 14.61 MB (11,579,920 + 3,736,464 bytes: `libflutter.so` + `libapp.so`) |
| `lib/armeabi-v7a/*.so` | 5.28 MB (5,534,276 bytes) | n/a (not in the arm64-only split) |
| `lib/x86_64/*.so` | 8.38 MB (8,789,176 bytes) | n/a (not in the arm64-only split) |
| `classes.dex` (total) | 0.13 MB (138,796 bytes) | 0.78 MB (821,848 bytes) |

Flutter's arm64 `.so` payload splits into two named pieces — `libflutter.so`
(11.58 MB, the **engine**: Skia/Impeller renderer + Dart VM runtime,
general-purpose and identical across any Flutter app) and `libapp.so`
(3.56 MB, the **AOT-compiled app snapshot**, this benchmark's actual
compiled Dart) — plus
a small `libdatastore_shared_counter.so` (~7 KB, `shared_preferences_android`
plugin support lib). Frust ships one `.so` per ABI with no such split: the
vello/wgpu render stack and the app's own logic compile straight into
`libfrustbench.so`, so there's no separately-shipped "engine" to measure.

### iOS release `.app` bundle

| App | Release `.app` (`du -sk`) | Profile `.app` (reference only — not release) |
|---|---|---|
| Frust | **7.96 MB** (8,156 KB) | n/a — the run-matrix builds Frust release-only on iOS (see the iPhone SE section above) |
| Flutter | **14.58 MB** (14,928 KB) | 22.10 MB (22,632 KB) |

**Build commands / provenance:**
- Frust: `frust build ios --release` (signed `FRUST_IOS_TEAM=87MFQ5L648`) —
  the existing campaign artifact under
  `benchmarks/frust_bench/build/ios/Build/Products/Release-iphoneos/Runner.app`
  (built 2026-07-20, the same release build the iPhone SE device matrix
  above used for S1–S8) — measured directly, not rebuilt this pass.
- Flutter: `flutter build ios --release` — **attempted and succeeded** this
  pass (Automatic signing; `DEVELOPMENT_TEAM = 87MFQ5L648` was already
  committed in the Xcode project, and a valid "Apple Development" signing
  identity for that team was present on this build host), producing
  `benchmarks/flutter_bench/build/ios/iphoneos/Runner.app` fresh. Flutter's
  pre-existing `Profile-iphoneos/Runner.app` (from the device campaign's
  per-scenario `--profile` builds) is shown only for reference — profile
  mode ships extra JIT-capable/tracing scaffolding and is a **different,
  larger config**, never compared head-to-head against Frust's release
  number.

### Narrative

**Frust's Android APK is roughly half Flutter's** (21.16 MB vs 43.51 MB
universal; 7.49 MB vs 15.48 MB arm64-only) and **its iOS `.app` is roughly
half Flutter's release build too** (7.96 MB vs 14.58 MB) — consistent with
the release profile's `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`
hardening (`docs/DEVELOPMENT.md`'s Release-profile hardening) producing one
self-contained, dead-code-eliminated native binary with no separate runtime
to ship. This is architectural, not merely a smaller optimization setting:
Frust's `.app` **is** its Rust binary almost in full (the `Runner` executable
alone is 8,260,736 bytes ≈ 7.88 MB of the 7.96 MB iOS bundle total — the
vello/wgpu render stack and UI logic all link straight into it), whereas
Flutter's `.app` bundles a general-purpose **engine**
(`Flutter.framework`, ~9.83 MB of the 14.58 MB iOS release total) *beside*
the app's own **AOT snapshot** (`App.framework`, ~4.36 MB) — a Skia/Impeller
renderer plus Dart VM shipped with every Flutter app regardless of what that
app does, a fixed cost no single app's LTO/tree-shaking pass can remove. The
same engine-vs-app split shows up on Android's arm64 breakdown above
(`libflutter.so` 11.58 MB vs `libapp.so` 3.56 MB). No build-config asymmetry
to flag on Android (release-vs-release, same-ABI-vs-same-ABI on both sides);
the one asymmetry the campaign initially had was iOS Flutter having only a
profile `.app` on disk — resolved this pass by successfully building
Flutter's release `.app` too, so both iOS numbers above are release vs.
release, with the earlier profile artifact kept only as a clearly-labeled
reference row.
