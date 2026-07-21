# Benchmark Results

Results record for the paired Frust-vs-Flutter benchmark suite, per
`PROTOCOL.md`. **A device or scenario with no completed runs is omitted, not
filled with placeholder numbers.**

Every table below must be reproducible from `benchmarks/raw/<device>/<scenario>/`
(raw per-run series, committed alongside this file — sanitized by
`harness/run.sh` to perf/marker lines only) via
`./benchmarks/harness/run.sh <scenario> --device <serial>`.

> **Results reset 2026-07-21.** All prior series (OnePlus 9 2026-07-20/21,
> iPhone SE 2026-07-20/21, and the phase-10 attribution re-run) were retired
> in one sweep — see git history for the retired tables. Reasons: (a) the
> **S5 layout-parity v2** fix (frust previously composited a ~66%-width cell
> vs Flutter's actual edge-to-edge render — PROTOCOL §8 S5 notes), (b) the
> build now carries the full phase-10 optimization set (shape cache,
> resampling, warmup, micro-wins) making earlier frust series stale, and
> (c) the device matrix changes to: **Xiaomi 12 (cupid, Snapdragon 8 Gen 1)**,
> **iPhone SE 2 (A13)**, and **OnePlus 9 (Adreno 660)** re-run later.
> Raw logs are now committed (sanitized); the pre-reset raws were never
> committed (old gitignore) and were deleted locally.

## Device: Xiaomi 12 (cupid, Snapdragon 8 Gen 1) — headline-tier Android

**Status:** run 2026-07-21, **full-form: 12 runs × 30s per scenario per app,
first 2 discarded (10 kept) — PROTOCOL §4 satisfied** (S7 used 60s runs for
the protocol's 60s idle window). **Frust = RELEASE build; Flutter = profile
(per protocol §2). Raw format v2** (`encode_us`+`present_us` split). First-ever
matrix pass on this device. Raw series (sanitized to perf/marker lines at
capture time by `run.sh`) committed under
`raw/xiaomi12/frust_release/<scenario>/run-NN.log` and
`raw/xiaomi12/flutter/<scenario>/run-NN.log`, plus per-run
`run-NN.pss_before/after.txt` meminfo snapshots.

- Chipset: Snapdragon 8 Gen 1 (SM8450) / Adreno 730
- Model: Xiaomi 12 (2201123G, codename cupid), serial `bd4775f2`
- OS: **LineageOS 23.2 (Android 16, SDK 36)** — nightly `23.2-20260604`,
  build `BP4A.251205.006`, security patch 2026-05-01, userdebug. NOT stock
  MIUI/HyperOS — no vendor battery-optimization overlays interfered; noted
  because vendor-ROM scheduling/thermal policy differs from stock.
- Display: 1080×2400 @ 440dpi, two modes (60Hz / 120Hz), **120Hz mode active
  for the whole session** (`mActiveSfDisplayMode` id=1 120.00001Hz). Both
  budgets (16.67ms / 8.33ms) therefore reported.
- High-refresh opt-in engaged on both apps (fairness gate §6): Frust
  `Surface.setFrameRate` hint, Flutter `flutter_displaymode`. Verified by
  achieved rates: Frust sustains ~119.7fps (S1) / ~120.5fps (S4); Flutter
  ~107.7fps (S1) / ~120fps (S4/S6). **Achieved-rate caveat:** the effective
  cadence is content/governor-dependent — Frust's S5/S6 settle at a ~88fps
  regime and Flutter's S1 at ~108fps despite the 120Hz panel mode; per-run
  frame counts are in the raw logs.
- Fixed brightness 128/255, auto-brightness disabled (`device_state.sh`).
- Airplane mode on (`settings put global airplane_mode_on 1`, read-back 1),
  Wi-Fi disabled (`svc wifi disable`, confirmed), Bluetooth disabled.
- Charger: physically USB-connected (adb requirement); `dumpsys battery
  unplug` presented an on-battery state to the DVFS/thermal governor,
  re-applied before every scenario block (run.sh's exit cleanup resets the
  spoof between blocks). `svc power stayon true` during blocks. All state
  restored + read-back confirmed after the session.
- Thermal: gate-based (`device_state.sh`, `dumpsys battery` temperature,
  ceiling 38°C) before each block. Session start 35.1°C, end-of-matrix
  32.5°C; **no forced cooldown wait ever triggered** — every one of the 16
  blocks completed in a uniform 349–350s (679s for the 60s S7 blocks),
  i.e. zero gate stalls (a cooldown wait would stretch the block wall time).
  Snapdragon 8 Gen 1 throttling did not visibly bite at this workload:
  per-run frame counts are flat across each block (e.g. S1 frust
  3578–3602 frames/run, run 1 → run 12).
- Flutter version: 3.44.2 stable — `flutter build apk --profile`
  (75.8 MB profile APK; package id `it.f0x.flutter_bench`, passed via
  `--pkg`).
- Frust build: **RELEASE** (`frust build apk --release`, 22,287,050-byte
  universal APK) — huddle upload keystore (`examples/huddle`
  `upload-keystore.jks`), JBR `JAVA_HOME`, NDK `27.0.12077973`.
  `FRUST_TRACE=1 FRUST_TRACE_RAW=1` exported in the build shell + a
  `crates/frust-shell-common/src/perf.rs` mtime-only touch to force the
  recompile past the release `--define` gap (long-standing recipe;
  `git status` stayed clean). Repo `main` @ `a55e09e` (includes the S5
  layout-parity v2 fix and the phase-10 optimization set). **v2 raw emission
  (`encode_us=`/`present_us=` split fields) confirmed on-device before any
  scored run**; stale differently-signed prior installs of both apps were
  uninstalled first.
- Visual gates: every scenario screencap-spot-checked on both apps before
  its capture block (no empty renders anywhere). **S5 mid-scroll parity
  explicitly verified: frust cells render edge-to-edge full width (8px side
  padding), identical geometry to Flutter's** — the layout-parity v2 fix
  holds on this device. Both S1 fields render the identical deterministic
  bubble layout (same tickers/positions on both apps).
- Sanitization: after every block, all raw logs were scanned — zero lines
  outside the `frust-perf|flutter-perf|bench-scenario|blank` whitelist in
  all 192 committed run logs; the `.unfiltered` full-logcat scratch was
  confirmed auto-deleted (none persists).

### S1 — Animation storm (spec v3, settle-capable field)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames (10 kept runs) |
|---|---|---|---|---|---|---|---|
| Frust (release) | **4.87** | **5.77** | **6.72** | 63.52 | **35 (0.10%)** | **212 (0.6%)** | 35,902 (~119.7 fps avg) |
| Flutter (profile) | 9.03 | 18.59 | 22.48 | **38.63** | 1,759 (5.4%) | 26,699 (82.6%) | 32,315 (~107.7 fps avg) |

**Frust wins S1 decisively on this device — the first clean S1 win in the
matrix' history.** Unlike the OnePlus 9's split (medians within noise), on
the Adreno 730 Frust holds a **4.9ms median at a locked ~120fps** (misses
the 120Hz budget on only 0.6% of frames) while Flutter's median is 9.0ms
with a p95 past the 60Hz budget (18.6ms) and 5.4% of frames dropping 60Hz.
Frust's only blemish is a taller single worst (63.5ms vs 38.6ms). At the
same v3 workload, vello's single-dispatch renderer runs this scenario at
roughly half Flutter/Impeller's per-frame cost across the whole
distribution — the plan's marquee S1 claim, previously proven only on
Adreno 840, now reproduces on Adreno 730. Neither app's field settled
within any 30s window (0 skipped frames on both) — no settled tail exists
in this dataset, same genuine finding as the OnePlus 9.

### S2 — Long-list scroll (10k rows)

Health-gated: frust `layout_us > 0` on 19,508/19,518 kept frames — a genuine
per-frame scroll relayout (the frozen-window failure mode does not affect
this pass).

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | **10.35** | **16.40** | **18.96** | 52.60 | **825 (4.2%)** | 14,390 | 19,518 (~65 fps continuous) |
| Flutter (profile) | 13.57 | 23.21 | 26.68 | **38.28** | 1,605 (24.8%) | 5,186 | 6,460 (event-driven, ~646/run) |

**Frust wins S2.** Median 10.4ms vs 13.6ms, p95 16.4ms vs 23.2ms, and a
~6× lower 60Hz-miss *rate* (4.2% vs 24.8% of frames attempted). Flutter's
lower absolute @8.33ms count reflects its ~3× smaller frame total
(event-driven paint), not better per-frame cost. Consistent with the
OnePlus 9's S2 verdict — no-GC scroll stability generalizes to this GPU
tier.

### S3 — Table ops (js-framework-benchmark subset, continuous cycling)

Cycle health confirmed on both apps (kept-run `bench-scenario-start
s3-<op>` reopen counts: frust ~31–32/run, flutter 18–19/run — continuous
cycling, non-empty table between the intentional settle gaps).

**Per-op reconcile-frame timing** (`s3-*` sub-markers aggregated across all
cycles in the 10 kept runs):

| Op | Frust p50/p95/p99/worst (ms, release) | n frames | Flutter |
|---|---|---|---|
| create 1k | 7.25 / 8.64 / 48.70 / 50.03 | 326 | **not captured** |
| create 10k | 13.62 / 21.52 / 23.84 / 25.36 | 325 | **not captured** |
| update every 10th of 10k | 10.90 / 13.53 / 18.99 / 20.18 | 324 | **not captured** |
| swap | 10.03 / 11.55 / 16.71 / 19.60 | 321 | **not captured** |
| clear | 11.41 / 12.81 / 20.14 / 23.26 | 320 | **not captured** |

**Flutter per-op capture landed zero frames in every sub-marker window**
across all 10 kept runs — the same structural
`SchedulerBinding.addTimingsCallback` async-delivery gap documented on both
pre-reset devices (timing data arrives after the synchronous marker-close;
a `flutter_bench` marker-placement fix remains the recorded follow-up, out
of a run-matrix task's scope). No cross-app per-op comparison exists on
this device either.

**Overall S3 frame series** (whole capture incl. settle gaps — *not*
apples-to-apples, continuous vs event-driven paint): Frust p50 4.87 / p95
9.99 / p99 13.17 / worst 50.03ms (32,255 frames, ~107.5fps continuous);
Flutter p50 13.81 / p95 30.07 / p99 34.64 / worst 39.58ms (922 frames,
~3.1fps event-driven — paints only at op transitions). No overall winner
declared from these, per the established convention; Frust's per-op table
is the clean result: every op's median sits inside a single 60Hz frame
(7.3–13.6ms), create-10k the heaviest, create-1k carrying a first-occurrence
tail outlier (p99 48.7ms) as on prior devices.

### S4 — Heavy-work responsiveness (~50MB JSON parse + concurrent animation)

| Metric | Frust (release) | Flutter (profile) |
|---|---|---|
| Total wall time (parse complete) | not captured (deviation 3) | not captured |
| Animation p50 during parse (ms) | 4.50 | **3.64** |
| Animation p95 during parse (ms) | 5.49 | **5.41** |
| Animation p99 during parse (ms) | 6.08 | **5.72** |
| worst (ms) | 59.36 | **15.99** |
| Missed-budget during parse (@16.67 / @8.33) | 12 / 41 | **0 / 16** |
| Active frames (10 kept runs) | 36,150 (~120.5 fps) | 35,999 (~120 fps) |

**Roughly a tie, Flutter marginally smoother** — the same verdict as the
OnePlus 9. Both apps hold a locked ~120fps animation while the heavy parse
runs off the UI thread (Frust `spawn_blocking`, Flutter `Isolate.run`);
Flutter's tail is tighter (worst 16.0ms vs 59.4ms).

### S5 — Image pipeline (decode-and-display while scrolling) — layout-parity v2

**First S5 series under the v2 full-width cell geometry** (both apps
compositing identical edge-to-edge cells — verified visually on-device, see
the metadata bullet). Health-gated: frust `layout_us > 0` on 26,562/26,572
kept frames — genuine scroll.

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | 5.72 | 12.29 | 13.42 | 53.16 | 35 | 7,968 | 26,572 (~88.6 fps) |
| Flutter (profile) | **5.29** | **9.29** | **11.21** | **19.27** | **2** | 4,218 | 34,671 (~115.6 fps) |

**Flutter wins S5 narrowly under the fair geometry.** Medians are close
(5.3 vs 5.7ms) but Flutter's p95/p99/worst are all tighter and it sustains
a higher frame rate at the same workload. Frust's pre-fix S5 numbers (which
composited ~⅓ fewer pixels) are retired and not comparable; this is the
honest post-fix result: near-parity at the median, Flutter steadier in the
tail on this device.

### S6 — Text shaping stress (multilingual relayout on width animation)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | **6.21** | 11.20 | 12.58 | 50.63 | 50 | 8,081 | 26,410 (~88 fps) |
| Flutter (profile) | 7.43 | **9.72** | **10.69** | **13.35** | **0** | 9,724 | 35,914 (~119.7 fps) |

**Split — Frust wins the median, Flutter wins the tail.** The phase-10
shape cache keeps Frust's typical shaping frame cheaper (6.2ms vs 7.4ms
p50, a 16% lead — the first device where Frust leads S6's median in a
same-day cross-app pass), but Flutter's distribution is much tighter
(p95 9.7 vs 11.2ms, worst 13.4 vs 50.6ms, zero dropped 60Hz frames) and it
holds ~120fps vs Frust's ~88fps cadence. Flutter's text pipeline is no
longer the clear S6 winner it was pre-shape-cache, but its consistency
still edges the tail.

### S7 — Cold start + idle (60s runs, protocol-compliant idle window)

Idle memory = mean TOTAL PSS over the 10 kept runs' post-run
`dumpsys meminfo` snapshots (committed alongside the logs). Idle CPU
sampled via `dumpsys cpuinfo` every 30s across both blocks.

| Metric | Frust (release) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of 3 kept of 4) | **~150 ms** (147/150/155) | ~368 ms (353/368/384) |
| Framework-reported first-frame span | ~87 ms median (`first_frame_presented`, 82–104 across 12 launches) | **~65 ms** median (`first_frame_ms`, 49–71) |
| Idle CPU, 60s window (avg %) | ~0% (0% in every idle sample) | ~0% (0% in every idle sample) |
| Idle memory (PSS, mean MB, 10 kept runs) | **~53.7 MB** (53.4–54.0) | ~89.6 MB (86.3–90.7) |

**Frust wins S7.** The external, identical-tool cold start is ~2.5× faster
(150ms vs 368ms) and idle memory is ~40% lower (53.7 vs 89.6MB PSS —
tighter run-to-run than the OnePlus 9's Flutter variance). The
framework-self-reported first-frame still favors Flutter (65 vs 87ms) as on
every prior device, but the gap is small here: Frust's GPU init is only
~29–40ms on this Adreno 730 (`adapter_ready`≈`renderer_ready`), and its
`first_frame_presented` median of 87ms is under half the OnePlus 9's
131.5ms. Both apps idle at 0% CPU (frame gate / event-driven idle).

### S8 — Plugin-call overhead (shared preferences)

Per-op median latency over the 10 kept runs (µs/call; first call per type
excluded as warmup; 1,990 samples per op per type per app). Frust
`bool`/`i64`/`f64`/`String`/`Vec<String>` ↔ Flutter
`bool`/`int`/`double`/`String`/`List<String>`. **Zero errors observed**: no
`s8-errors` marker in any of the 24 logs (kept or discarded), both apps —
every write `Result` ok, every read verified against its expected value
(error-accounting + read-timing-symmetry fixes are in both binaries).

| Op | Frust (µs/call, release) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | **97** | 288 | n/a |
| write i64 | **75** | 285 | n/a |
| write f64 | **70** | 300 | n/a |
| write String | **68** | 228 | n/a |
| write Vec\<String\> | **69** | 231 | n/a |
| read (unique key, forces channel) bool | **20** | 5133† | ~0 |
| read (unique key, forces channel) i64 | **19** | 5133† | ~0 |
| read (unique key, forces channel) f64 | **20** | 5133† | ~0 |
| read (unique key, forces channel) String | **20** | 5133† | ~0 |
| read (unique key, forces channel) Vec\<String\> | **20** | 5133† | ~0 |

**Frust wins S8 decisively — again the clearest, most robust
Rust-advantage result** (~3.0–4.2× on writes, the boundary-crossing
headline; same structural margin as both pre-reset devices). †Flutter's
only channel-crossing read is the package's `reload()` whole-store re-read
(median 5,133µs, one figure for all read rows); Frust's read is a real
per-key backend call at ~20µs. Flutter's cached read (~0µs) is a Dart-map
lookup, not a boundary crossing. The burst-during-animation variant was not
run this pass (deviation 4).

### Methodology deviations (this device)

1. **Custom ROM.** The device runs LineageOS 23.2 (Android 16, userdebug),
   not stock MIUI/HyperOS — scheduler/thermal policy may differ from a
   stock Xiaomi 12. Both apps ran under the identical ROM, so the cross-app
   comparison is unaffected; cross-device absolute comparisons should note
   it. (Upside: no vendor battery-optimization popups/overlays interfered;
   none appeared at any point.)
2. **Flutter release-mode in-app cross-check (PROTOCOL §2) not captured** —
   only the profile-mode headline series was taken.
3. **S4 parse wall-time not recoverable** — the capture is timestamp-free
   (`logcat -v raw`) and neither app emits a parse-duration line; only the
   animation-during-parse percentiles are reported (same as prior devices).
4. **S8 burst-during-animation variant not run** this pass (matches both
   pre-reset device passes).
5. **S3 Flutter per-op capture landed zero frames** in every sub-marker
   window — structural `addTimingsCallback` async-delivery gap, third
   device in a row; `flutter_bench` marker-placement fix remains the
   recorded follow-up.
6. **External cold start** measured post-matrix (4 launches/app, first
   discarded, 3s force-stop gap) rather than inside the run.sh loop; S7's
   in-loop launches provided the 12 framework-span samples.
7. **Charger:** physically USB-connected throughout (adb); `dumpsys battery
   unplug` spoofed on-battery state identically for both apps (re-applied
   per block). Battery 57%→100% real charge over the session despite the
   spoof (physical charging continues under the spoof; the governor sees
   on-battery).
8. **Frust release trace-injection quirk** (known recipe): release
   `--define` does not reach the cargo compile; `FRUST_TRACE=1
   FRUST_TRACE_RAW=1` exported in the build shell + `perf.rs` mtime touch.
   Verified v2 emission on-device before scoring; working tree stayed
   clean.
9. **Frust S2/S5/S6 run at a ~65–88fps cadence** (not the panel's 120) —
   an honest achieved-rate observation, not a capture defect: per-frame
   cost (~10.4ms S2) or the governor's chosen cadence bounds the loop; all
   percentile math is per-frame and budget-referenced, unaffected by the
   attempted rate. Flutter's S1 similarly ran ~108fps.

---

## App size (release) — re-measured post-reset

Host-side snapshot via `benchmarks/harness/app_size.sh`, 2026-07-21, after
the phase-10 build set (supersedes the retired pre-reset numbers; Android
frust universal grew ~0.1 MB vs the retired 22,188,018-byte figure).

| Axis | Frust | Flutter |
|---|---|---|
| Android universal release APK (3 ABIs) | **21.25 MB** (22,287,050 B) | 43.51 MB (45,628,582 B) |
| Android arm64-v8a split | not rebuilt this pass | 15.48 MB (16,236,261 B) |
| arm64 `.so` payload (from universal) | 7.42 MB | 14.6 MB (engine+app, split APK) |
| dex total | 0.13 MB | — |
| iOS release `.app` (`du -sk`, from disk) | 7.96 MB | 22.10 MB† |

†The Flutter iOS artifact currently on disk measures 22.10 MB — the
pre-reset 14.58 MB release figure came from a fresh `flutter build ios
--release` at that time; the on-disk bundle has since been overwritten (see
git history). Android release-vs-release remains the clean comparison:
**frust ~half of Flutter on every Android axis**, consistent with the
retired snapshot's narrative.
