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
§2).** Flutter columns are the original profile pass; the Frust column is a
post-fix RELEASE re-run (commit `ade0d87`). Raw series under
`raw/oneplus9/frust_release/<scenario>/runN.log` (Frust release) and
`raw/oneplus9/<app>/<scenario>/runN.log` (original pass; gitignored — full-device
logcat is not committed).

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

> **⚠️ S1 PARITY-SUSPECT (both apps) — NO WINNER DECLARED.** The two apps are not
> running an identical workload: the user observed Frust's bubbles render much
> larger than Flutter's (so Frust has constant collisions / churn while Flutter's
> quickly settle), and Frust renders edge-to-edge so its layout bounds differ.
> Until the bubble size/physics/bounds are matched and re-run, these numbers do
> not compare like for like — recorded for reference only. A parity fix + re-run
> is queued.

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) — parity-suspect | 19.55 | 23.64 | 24.92 | 56.00 | 2778 | 3733 |
| Flutter (profile) — parity-suspect | 9.24 | 16.99 | 20.53 | 49.65 | 516 | 7771 |

Numbers show Frust ~19.6ms median vs Flutter ~9.2ms, but **because the workloads
differ (see banner) this is not a valid comparison** — no winner. (Release did not
move Frust's S1 vs the earlier profile pass: 19.55 vs 19.58ms — S1 is GPU-raster
bound, where LTO doesn't help.) The plan's S1 vello-vs-Impeller claim, proven on
the headline Adreno 840, remains untested here pending a parity-matched re-run.

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

> **⚠️ Frust S3 SUSPECT.** The user observed S3 visually glitching on the Frust
> side; a fix is under investigation. Numbers below are kept but should be treated
> as provisional until that fix lands and S3 is re-run.

Per-op reconcile-frame total time (`s3-*` sub-markers), Frust = release. **Frust
captured all five ops; Flutter's per-op sub-marker windows captured only `create1k`
reliably** (the other ops' reconcile frames landed outside their marker windows —
see deviations). Frust's first op (`create1k`) carries first-frame/warmup cost.

| Op | Frust (ms, release) | Flutter (ms) |
|---|---|---|
| create 1k | 52.88 | 19.28 |
| create 10k (setup for update) | 7.03 | not captured |
| update every 10th of 10k | 7.18 | not captured |
| swap | 8.50 | not captured |
| clear | 13.02 | not captured |

Overall S3 frame series (marker `s3`): Frust release p50 7.63ms / p95 19.98ms (246
active, 6469 frame-gate-skipped); Flutter p50 15.51ms / p95 29.35ms (19 active).
Frust's steady-state reconciles (7–13ms) look faster than Flutter's overall p50,
but with the SUSPECT flag above and incomplete Flutter per-op capture, S3 is
**inconclusive** — no winner declared.

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

Per-op median latency over kept runs (µs/call), Frust = release, computed from each
app's `*-perf plugin op=… us=…` lines. Frust value types `bool`/`i64`/`f64`/
`String`/`Vec<String>` map to Flutter `bool`/`int`/`double`/`String`/`List<String>`.

| Op | Frust (µs/call, release) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | **101** | 290 | n/a |
| write i64 | **96** | 285 | n/a |
| write f64 | **69** | 316 | n/a |
| write String | **67** | 261 | n/a |
| write Vec\<String\> | **69** | 285 | n/a |
| read (unique key, forces channel) bool | **18** | 4644† | ~0 |
| read (unique key, forces channel) i64 | **18** | 4644† | ~0 |
| read (unique key, forces channel) f64 | **18** | 4644† | ~0 |
| read (unique key, forces channel) String | **18** | 4644† | ~0 |
| read (unique key, forces channel) Vec\<String\> | **19** | 4644† | ~0 |

**Frust wins S8 decisively** — the clearest and most robust Rust-advantage result.
Writes (which always cross the boundary on both sides): Frust ~67–101µs vs Flutter
~261–316µs — a **~2.6–4.6× advantage** for direct in-process FFI (`jni`, zero
codec) over Flutter's `StandardMethodCodec` MethodChannel round-trip. †Flutter's
only channel-crossing *read* is the package's `reload()` path, which re-reads the
**entire** store in one hop (median 4644µs, p95 ~16.6ms) — not a per-type single
read, so the same figure is shown for every read row; Frust's read is a real
per-key backend call at ~17µs (no Dart-style cache). Flutter's cached read is a
Dart-memory map lookup (~0µs) — genuinely fast, but not a boundary crossing.

| Burst-during-animation variant | Frust | Flutter |
|---|---|---|
| Animation p95 during S8 burst (ms) | not run this pass | not run this pass |
| Animation missed-budget count during S8 burst | not run this pass | not run this pass |

The burst variant (`FRUST_BENCH_S8_BURST=1` / `--dart-define=S8_BURST=1`) was not
included in this short-form pass (deviation).

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
4b. **S1 PARITY-SUSPECT (both apps) — no winner.** The two apps' S1 workloads are
   not identical (Frust's bubbles are visibly larger → constant collisions vs
   Flutter's quick settle; Frust renders edge-to-edge → different bounds). Numbers
   are recorded for reference only; a parity fix + re-run is queued.
4c. **S3 SUSPECT (Frust).** Visual glitching observed on the Frust side; a fix is
   under investigation. S3 numbers are kept but provisional; no winner declared.
4. **S3 per-op: Flutter capture incomplete.** Only `s3-create1k` landed a frame
   inside its sub-marker window on the Flutter side; `create10k`/`update`/`swap`/
   `clear` reconcile frames fell outside their windows (marker/frame-timing
   granularity), so those Flutter per-op cells read "not captured." Frust captured
   all five.
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

---

## Device: iPhone SE — iOS

**Status:** SKIPPED this pass (2026-07-20) — no iOS codesigning identity at desk,
so a physical-device build cannot be signed/installed (recorded as pending; iOS
runs are the manual-capture flow, `run.sh --device ios`). Tables left as the empty
template below.

- OS version: _(fill in)_
- Fixed brightness: _(fill in)_
- Refresh-rate mode: 60Hz (iPhone SE has no ProMotion panel — the 8.33ms
  budget column is N/A for every scenario on this device)
- Flutter version: 3.44.2 stable
- Frust build: _(commit / release profile confirmation)_
- Thermal-cooldown method: _(fill in)_

### S1 — Animation storm

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | | | | | N/A (60Hz panel) |
| Flutter (profile) | | | | | N/A (60Hz panel) |

### S2 — Long-list scroll (10k rows)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | | | | | N/A (60Hz panel) |
| Flutter (profile) | | | | | N/A (60Hz panel) |

### S3 — Table ops (js-framework-benchmark subset)

| Op | Frust (ms) | Flutter (ms) |
|---|---|---|
| create 1k | | |
| update every 10th of 10k | | |
| swap | | |
| clear | | |

### S4 — Heavy-work responsiveness (~50MB JSON parse + concurrent animation)

| Metric | Frust | Flutter |
|---|---|---|
| Total wall time (parse complete) | | |
| Animation p50 during parse (ms) | | |
| Animation p95 during parse (ms) | | |
| Animation p99 during parse (ms) | | |
| Animation missed-budget count during parse | | |

### S5 — Image pipeline (decode-and-display while scrolling)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms |
|---|---|---|---|---|---|
| Frust (release) | | | | | |
| Flutter (profile) | | | | | |

### S6 — Text shaping stress (multilingual relayout on width animation)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms |
|---|---|---|---|---|---|
| Frust (release) | | | | | |
| Flutter (profile) | | | | | |

### S7 — Cold start + idle

| Metric | Frust | Flutter |
|---|---|---|
| External cold start (`xcrun`/Instruments launch time) | | |
| Framework-reported first-frame span | | |
| Idle CPU, 60s (avg %) | | |
| Idle memory, 60s (RSS, avg MB) | | |

### S8 — Plugin-call overhead (shared preferences)

| Op | Frust (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | | | n/a |
| write i64 | | | n/a |
| write f64 | | | n/a |
| write String | | | n/a |
| write Vec\<String\> | | | n/a |
| read (unique key, forces channel) bool | | | |
| read (unique key, forces channel) i64 | | | |
| read (unique key, forces channel) f64 | | | |
| read (unique key, forces channel) String | | | |
| read (unique key, forces channel) Vec\<String\> | | | |

| Burst-during-animation variant | Frust | Flutter |
|---|---|---|
| Animation p95 during S8 burst (ms) | | |
| Animation missed-budget count during S8 burst | | |

### Methodology deviations (this device)

_(As above.)_

---

## Device: OnePlus 15 (Adreno 840) — headline Android

**Status:** pending device availability — not yet run. This column is the
plan's headline device (proven S1 Impeller-vs-vello win on Adreno 840,
now to be formalized across the full matrix); added here whenever the
device is next at desk. Copy the OnePlus 9 section's table shapes above
when filling this in.

---

## Cross-device narrative

Only one device (OnePlus 9 / Adreno 660, mid-tier Android) has a matrix so far;
the headline Adreno 840 (OnePlus 15) and the iPhone SE remain pending, so this is
a single-tier snapshot, not the cross-tier story. On this **mid-tier** device, in
short form (5×30s), **Frust = release** (huddle keystore), **Flutter = profile**:

- **Frust wins:** **S2** (long-list scroll — valid post-fix release re-run: ~11.0ms
  vs ~15.3ms median, 104 vs 666 dropped-60Hz frames); **S8** (plugin-call overhead —
  writes ~2.6–4.6× faster, channel-crossing reads 18µs vs 4644µs — the clearest,
  most robust Rust-advantage result); **S7 external cold start** (`am start -W`
  ~215ms vs ~417ms) and **idle memory** (~98MB vs ~126MB PSS).
- **Flutter wins:** **S6** (text shaping — ~8.3ms vs ~10.7ms median); **S7
  framework-reported first-frame** (84ms vs ~147ms — Frust's GPU/renderer init is
  heavier).
- **Roughly even / split:** **S4** (heavy-work — both hold frame rate off-thread,
  Flutter marginally smoother tail); **S5** (image pipeline, valid post-fix — Flutter
  lower median 6.2 vs 9.9ms, Frust tighter p95/p99 and fewer drops).
- **No winner (workload/behavior suspect):** **S1** (animation storm — PARITY-SUSPECT:
  Frust bubbles larger + edge-to-edge, so not the same workload; a parity fix + re-run
  is queued); **S3** (table ops — Frust visually glitching, SUSPECT, fix under
  investigation). Both are recorded but not scored.

The honest headline for this device: **Frust's demonstrated advantages are at the
plugin boundary (S8), in long-list scroll (S2), and in cold-start/idle-footprint
(S7); Flutter's mature text pipeline wins S6 and reaches its own first frame
sooner.** S1 (animation storm) and S3 (table ops) are not yet comparable — S1 needs
a workload-parity fix, S3 a rendering fix — and the plan's marquee S1
vello-vs-Impeller claim is Adreno-840-specific, to be re-measured on the OnePlus 15.
Moving Frust from profile to a true `--release` build changed the per-frame and
plugin numbers only within noise (S1/S4/S6/S7/S8) — the wins/losses are structural,
not a build-mode artifact.
