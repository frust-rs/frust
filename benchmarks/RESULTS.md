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

## Device: iPhone SE 2 (A13 Bionic) — iOS

**Status:** run 2026-07-21, **full-form: 12 runs × 30s per scenario per app,
first 2 discarded (10 kept) — PROTOCOL §4 satisfied** (see deviation 12 for
why S7 also used 30s runs on this platform). **Frust = RELEASE build; Flutter
= profile (per protocol §2). Raw format v2** (`encode_us`+`present_us`
split — its first iOS outing, and it does what it was built for: the
CADisplayLink present-to-vsync wait that made the retired 2026-07-21
pre-reset iPhone series' totals bimodal is now isolated in `present_us`,
leaving per-pass *work* directly readable — see the S1–S6 work rows and
`ATTRIBUTION.md`'s iOS section). Raw series (whitelist-filtered at capture
by `run.sh`) committed under `raw/iphone_se/frust_release/<scn>/run-NN.log`
and `raw/iphone_se/flutter/<scn>/run-NN.log` (192 logs, all verified to
contain only perf/marker/blank lines).

- Chipset: Apple A13 Bionic (iPhone SE 2nd gen, `iPhone12,8`), UDID
  `B64D62E7-A836-5BB6-ABC3-354BE21C9F19`
- OS: iOS 26.5.2
- Display: 60Hz panel (no ProMotion) — **the 8.33ms budget column is N/A for
  every scenario on this device** (protocol §5); both apps sustain ~60–62 fps
  on every continuously-rendering scenario.
- Flutter: 3.44.2 stable, `flutter build ios --profile
  --dart-define=SCENARIO=<sN>` (one build per scenario, 8 builds; team
  `87MFQ5L648` from the committed Xcode project;
  `CADisableMinimumFrameDurationOnPhone` already in Info.plist — fairness
  gate §6 satisfied).
- Frust: **RELEASE** (`frust build ios --release`, signed
  `FRUST_IOS_TEAM=87MFQ5L648`), one build for all scenarios, repo `main`
  @ `82dc949` (S5 layout-parity v2 + full phase-10 optimization set).
  `FRUST_TRACE=1 FRUST_TRACE_RAW=1` baked via `--define` (the iOS
  run-script's `FRUST_DEFINES`/`option_env!` path — no `perf.rs` touch
  needed on iOS, unlike Android). **v2 raw emission
  (`encode_us=`/`present_us=` split + `first_encode_done` startup span)
  confirmed on-device in a scratch smoke before any scored run.**
- Scenario selection/transport per app as documented in `run.sh`'s header:
  frust via `devicectl ... launch -e '{"FRUST_BENCH_SCENARIO":...}'` +
  `--console` stdout; Flutter via the compile-time define + the
  `tmp/flutter_bench_trace.log` container pull. Same `stats.py` slices both.
- Environmental controls: **brightness / airplane mode / charger / thermal
  all uncontrolled — iOS exposes no CLI for any of them** (deviation 8).
  Cooldown = fixed 60s between the two app blocks of each scenario plus the
  ~1min install/setup gap between scenarios. No thermal probe exists on this
  host; the throttling evidence is indirect and negative: per-run frame
  counts are flat within every block across the ~2h session (e.g. frust S1
  1798→1817 frames/run run 1→12, S6 1810→1823; flutter S1 1845–1860), and
  per-pass work medians are stable block-to-block — no heat-soak drift
  visible at this workload despite the A13-in-SE-body concern.
- Visual gate: **no CLI screenshot exists on this host** (Xcode 26
  `devicectl` has no screenshot subcommand; no libimobiledevice) — the S5
  mid-scroll eyeball is deferred to the user (deviation 13). In its place
  every block was gated on per-run raw-frame-line counts and plausibility
  (all 16 blocks uniform; no empty-render signature anywhere; S5 iOS
  relayouts every frame — `layout_us > 0` on 18,168/18,168 kept frames —
  so the Android layout-skip empty-render class cannot silently apply).

**Read deviations 1–2 before the frame tables** (unchanged from the retired
iPhone series): Flutter's frame total is the **`build+raster` work-sum**
(`totalSpan` is negative under load on iOS — up to 18,367 of 18,537 frames
in S4), and Frust's `total_us` folds in the CADisplayLink present-to-vsync
wait (its p95 sits at ~16.5–17.2ms — the 60Hz cadence — on every
continuously-rendering scenario). **New this pass: the v2 split makes
Frust's real per-frame work directly readable** (work =
rebuild+layout+paint+encode; `present_us` p50 is the real present cost,
its p95 the vsync wait), so each render table carries a Frust "work" row.
Per the established convention no per-frame render winner is declared from
the asymmetric totals; the work-vs-work rows are the informative
comparison, and S7/S8 remain the cleanly cross-comparable results.

### S1 — Animation storm (spec v3)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust total_us† (release) | 5.76 | 16.61† | 16.75† | 40.24 | 442† | N/A | 18,157 (~60.5 fps) |
| Frust **work** (rebuild+layout+paint+encode) | **~4.41** | — | — | — | — | N/A | — |
| Flutter work-sum (profile) | 4.92 | 5.50 | 5.87 | 25.26 | 7 | N/A | 18,526 (~61.8 fps) |

†vsync-wait artifact (present_us p95 = 12.9ms; its p50 is only 0.82ms).
Frust work p50: encode 4.09ms + rebuild 0.11 + paint 0.19 + layout 0.02.
Both sustain ~60–62 fps; **near-parity on real work (frust ~4.4ms vs
flutter ~4.9ms p50)** — no winner declared (metric asymmetry), but unlike
the retired pass the split now shows frust's S1 work is *not* behind.
0 skipped frames (field never settles in 30s, same as every device).

### S2 — Long-list scroll (10k rows)

Health-gated: frust `layout_us > 0` on 18,150/18,150 kept frames (genuine
per-frame relayout; layout p50 2.52ms is the dominant CPU pass).

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust total_us† (release) | 5.85 | 17.23† | 17.38† | 39.51 | 1,735† | N/A | 18,150 (continuous ~60.5 fps) |
| Frust **work** | **~4.62** | — | — | — | — | N/A | — |
| Flutter work-sum (profile) | 4.87 | 5.43 | 5.91 | 11.13 | 0 | N/A | 6,415 (event-driven, ~641/run) |

Work-vs-work essentially even (4.6 vs 4.9ms p50); frust renders
continuously (~1815 frames/run) vs Flutter's event-driven scroll — same
shape as every prior device.

### S3 — Table ops (continuous cycling)

Cycle health confirmed both apps (kept-run `s3-create1k` reopen counts:
frust 19/run, flutter 15/run). Frust per-op reconcile-frame timing — on
this 60Hz device every per-op *total* is vsync-locked at ~16ms, so the
informative per-op number is the **work decomposition** (µs, p50 over all
cycles in the 10 kept runs):

| Op | rebuild | layout | paint | encode | **work Σ p50 (ms)** | total p50 (ms) | n frames | Flutter |
|---|---|---|---|---|---|---|---|---|
| create 1k | 1,271 | 250 | 238 | 3,565 | **~5.3** | 16.04 | 180 | **not captured** |
| create 10k | 7,007 | 170 | 131 | 2,900 | **~10.2** | 15.88 | 183 | **not captured** |
| update 10th of 10k | 3,290 | 53 | 84 | 3,151 | **~6.6** | 16.03 | 180 | **not captured** |
| swap | 2,628 | 55 | 85 | 3,224 | **~6.0** | 16.08 | 180 | **not captured** |
| clear | 3,457 | 14 | 30 | 2,757 | **~6.3** | 16.04 | 180 | **not captured** |

**Flutter per-op capture landed zero frames in every sub-marker window**
across all 10 kept runs — the same structural
`addTimingsCallback` async-delivery gap, now the **fourth consecutive
device pass**; the `flutter_bench` marker-placement fix remains the
recorded follow-up. Whole-series (not apples-to-apples, continuous vs
event-driven): frust total p50 16.22ms at ~60fps continuous (18,178
frames); flutter work-sum p50 4.97 / p95 12.33ms over 960 event-driven
frames (~73–99/run). Every frust op's work fits comfortably inside a 60Hz
frame — create-10k heaviest at ~10.2ms, dominated by rebuild (7.0ms).

### S4 — Heavy-work responsiveness (~50MB JSON parse + animation)

| Metric | Frust (release) | Flutter (profile) |
|---|---|---|
| Total wall time (parse complete) | not captured (deviation 4) | not captured |
| Animation p50 (ms) | 16.29† total / **~3.57 work** | 1.34 (work-sum) |
| Animation p95 (ms) | 16.69† / — | 1.59 |
| Animation p99 (ms) | 16.98† / — | 1.71 |
| worst (ms) | 37.97 | 7.63 |
| missed @16.67ms | 1,072† | 0 |
| Active frames (10 kept) | 18,197 (~60.6 fps) | 18,537 (~61.8 fps) |

†fully vsync-locked (present_us p50 12.7ms — the animation is cheap and
waits for the 60Hz tick every frame; work = 0.10 rebuild + 3.42 encode +
~0.05 layout/paint ≈ 3.6ms). **Both apps hold a locked ~60fps animation
through the whole parse** (`spawn_blocking` vs `Isolate.run`); Flutter's
work is lower (1.3 vs 3.6ms). 18,367 of Flutter's 18,537 totalSpans were
negative here — the work-sum convention is doing all the lifting.

### S5 — Image pipeline (decode + scroll) — layout-parity v2

First iOS S5 series under the v2 full-width geometry (frust composites the
same edge-to-edge `(viewport−16)×256` cells as Flutter). Health:
`layout_us > 0` on 18,168/18,168 kept frames. **No on-device visual parity
check was possible this pass (no CLI screenshot — deviation 13); the v2
geometry was visually verified on Android same-day.**

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust total_us† (release) | 5.51 | 16.54† | 16.75† | 39.98 | 334† | N/A | 18,168 (~60.6 fps) |
| Frust **work** | **~4.67** | — | — | — | — | N/A | — |
| Flutter work-sum (profile) | **2.71** | **3.61** | **3.90** | **5.52** | 0 | N/A | 18,483 (~61.6 fps) |

**Flutter's work is markedly lower here (2.7 vs 4.7ms p50)** — consistent
with the Xiaomi 12's post-fix verdict (Flutter wins S5 under the fair
geometry) and with the retired iPhone series' shape. Frust's cost is
encode-dominated (3.95ms of the 4.7); both sustain ~60fps with no real
drops.

### S6 — Text shaping stress

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust total_us† (release) | 5.51 | 16.48† | 16.64† | 40.07 | 128† | N/A | 18,188 (~60.6 fps) |
| Frust **work** | **~4.67** | — | — | — | — | N/A | — |
| Flutter work-sum (profile) | 4.24 | 4.96 | 5.26 | 16.80 | 1 | N/A | 18,563 (~61.9 fps) |

Near-parity (frust work ~4.7 vs flutter 4.2ms p50, ~11% apart). The
phase-10 shape cache holds on iOS: frust layout (re-shaping) p50 is only
0.20ms — the frame is encode-bound (4.07ms), not text-bound, matching the
Android attribution finding.

### S7 — Cold start + idle

Framework-reported spans over **all 12 cold launches per app** (every run
is a `--terminate-existing` cold launch; iOS has no `am start -W`
equivalent, no idle-CPU and no RSS CLI — deviations 5–6, so the external
cold start and both idle axes are n/a on this platform).

| Metric | Frust (release) | Flutter (profile) |
|---|---|---|
| External cold start | n/a (no iOS CLI) | n/a |
| Framework first-frame span, median (min–max) | ~85 ms (69–190) `first_frame_presented` | **~7 ms** (6–20) `first_frame_ms` |
| Idle CPU | n/a (no iOS CLI) | n/a |
| Idle memory | n/a (no iOS CLI) | n/a |

Frust span decomposition (medians of 12): adapter/device/renderer_ready
~22ms → font_preinit_joined 71ms → first_rebuild_done 71ms →
first_encode_done 83ms → presented 85ms. **The retired iPhone series'
~188ms is down to ~85ms** (phase-10 warmup set) — the first-frame floor on
iOS is now the ~49ms font/TextContext preload, which (unlike Android,
where its measured delta is 0) does **not** fully overlap GPU init here;
first-frame *encode* is only ~12ms on the A13 (vs ~71ms on Adreno 660).
Flutter's self-reported `main()`→first-frame (~7ms) measures a much
narrower span than frust's app-entry→presented — the two are not the same
quantity, and with no external identical-tool measurement possible on iOS,
S7 has no cross-framework winner this pass (the Android external metric
favored frust ~2.5× on the Xiaomi 12). One qualitative outlier: the very
first launch after a fresh install (scratch smoke, unscored) read
font_preinit_joined=339ms / presented=388ms — cold OS font caches;
scored launches never exceeded 190ms.

### S8 — Plugin-call overhead (shared preferences)

Per-op median latency over the 10 kept runs (µs/call; first call per type
excluded; **1,990 samples per op per type per app**). **Zero errors**: no
`s8-errors` marker and `err=0` on every line, all 24 logs, both apps.

| Op | Frust (µs/call, release) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | **3** | 132 | n/a |
| write i64 | **3** | 54 | n/a |
| write f64 | **4** | 55 | n/a |
| write String | **3** | 53 | n/a |
| write Vec\<String\> | **5** | 60 | n/a |
| read (forces channel) bool | **1** | 2,673† | ~0 |
| read (forces channel) i64 | **1** | 2,673† | ~0 |
| read (forces channel) f64 | **1** | 2,673† | ~0 |
| read (forces channel) String | **1** | 2,673† | ~0 |
| read (forces channel) Vec\<String\> | **1** | 2,673† | ~0 |

**Frust wins S8 decisively — writes ~12–44× faster** (3–5µs direct
`objc2`/`NSUserDefaults` FFI vs 53–132µs MethodChannel round-trip),
reproducing the retired iPhone series' ~13–30× at full-form sample counts.
†Flutter's only channel-crossing read is the package's whole-store
`reload()` (2,673µs, one figure for all rows); frust's per-key read is
~1µs. Burst-during-animation variant not run (deviation 7).

### Methodology deviations (this device)

1. **Flutter frame total = `build_us+raster_us` work-sum** — `totalSpan`
   is negative under load on iOS (S4: 18,367 of 18,537 kept frames), as
   first documented in the retired iPhone series. Raw `total_us` remains
   in the committed lines for inspection.
2. **Frust `total_us` folds in the CADisplayLink present-to-vsync wait** —
   p95 ≈ 16.5–17.2ms on every continuous scenario is cadence, not cost.
   New this pass: raw v2 isolates the wait in `present_us` (p50 0.26–0.82ms
   real present cost, p95 12.5–13.6ms wait), so per-pass work is reported
   alongside and the † totals are auditable per-frame in the raws.
3. **Frust ran `--release`; Flutter profile** (protocol §2). The Flutter
   release-mode in-app cross-check was again not captured.
4. **S4 parse wall-time not recoverable** (timestamp-free capture; no
   parse-duration line — same as every prior device).
5. **No external cold-start tool exists on iOS** — S7's identical-tool
   comparison is impossible; only framework-self-reported spans captured.
6. **No idle-CPU or memory CLI on iOS** — both S7 idle axes are n/a (the
   protocol's PSS axis is Android-only; RSS would not be cross-comparable
   anyway per RESEARCH.md §8).
7. **S8 burst-during-animation variant not run** (matches all prior passes).
8. **Environmental controls uncontrolled** (no iOS CLI for brightness/
   airplane/charger/thermal); cooldown was a fixed 60s inter-app-block wait
   plus inter-scenario install gaps; no thermal sensor readout exists —
   flat per-run frame counts across every block are the (negative)
   throttling evidence.
9. **Scenario selection and capture transport differ per app** (env-var +
   console vs dart-define + container pull — both automatic; identical
   `stats.py` math over identical line formats).
10. **Harness fix required mid-session (uncommitted):** Xcode 26's
    `devicectl` dropped `process terminate --pid-of` — `run.sh`'s
    `ios_terminate` was silently a no-op, leaving apps running after
    captures and truncating console flushes. Fixed in `run.sh` by
    resolving the pid via `device info apps`/`processes` JSON + `--pid`,
    plus a bounded (≤15s) wait for the `--console` stream to drain after
    terminate. Validated before any scored run; all 192 committed logs
    post-date the fix.
11. **Popup incident (pre-scoring only).** A system dialog appeared on the
    device during the scratch smoke validation and paused the foreground
    app (~4s into a 10s smoke); the user dismissed it. Zero scored runs
    were affected (all scored blocks started after dismissal and every
    run's frame count was audited — no gap signature anywhere).
12. **S7 used 30s runs** (not the Xiaomi pass's 60s): the 60s length only
    serves the idle CPU/memory window, which is unmeasurable on iOS
    (deviation 6); 12 cold launches per app supplied the startup spans.
13. **No CLI screenshot on this host** — per-scenario visual spot-checks
    and the S5 mid-scroll parity eyeball could not be performed; deferred
    to the user. Plausibility was gated on frame-line counts, layout-health
    (`layout_us > 0`), marker structure, and cross-run uniformity instead.

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
