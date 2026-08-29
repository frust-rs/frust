# Benchmark Results

Results record for the paired Frust-vs-Flutter benchmark suite, per
`PROTOCOL.md`. **A device or scenario with no completed runs is omitted, not
filled with placeholder numbers.**

Every table below must be reproducible from `benchmarks/raw/<device>/<scenario>/`
(raw per-run series, committed alongside this file — sanitized by
`harness/run.sh` to perf/marker lines only) via
`./benchmarks/harness/run.sh <scenario> --device <serial>`.

> **Results reset 2026-07-21.** All prior series (OnePlus 9 2026-07-20/21,
> iPhone SE 2026-07-20/21, and the optimization-attribution re-run) were retired
> in one sweep — see git history for the retired tables. Reasons: (a) the
> **S5 layout-parity v2** fix (frust previously composited a ~66%-width cell
> vs Flutter's actual edge-to-edge render — PROTOCOL §8 S5 notes), (b) the
> build now carries the full shape-cache/resampling/warmup/micro-optimization
> set making earlier frust series stale, and
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
  layout-parity v2 fix and the shape-cache/resampling/warmup optimization set).
  **v2 raw emission
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
distribution — the marquee S1 claim under test, previously proven only on
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
of scope for these benchmark runs). No cross-app per-op comparison exists on
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

**Split — Frust wins the median, Flutter wins the tail.** The
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
10. **S8 per-op numbers predate shared-script parsing.** The per-op
    medians in the S8 table above were computed outside `stats.py` — the
    shared script has never parsed a per-op `plugin`/`op` line (PROTOCOL
    §7's per-op-line-format methodology-deviations note), only `*-perf
    raw` frame lines and `bench-scenario-*` markers. Reconciling S8's
    per-op parsing into the shared script is the harness task's job;
    these numbers remain individually reproducible from the committed raw
    logs but were not run through the single shared statistics script §5
    otherwise mandates.

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
  `<team-id>` from the committed Xcode project;
  `CADisableMinimumFrameDurationOnPhone` already in Info.plist — fairness
  gate §6 satisfied).
- Frust: **RELEASE** (`frust build ios --release`, signed
  `FRUST_IOS_TEAM=<team-id>`), one build for all scenarios, repo `main`
  @ `82dc949` (S5 layout-parity v2 + the full shape-cache/resampling/warmup
  optimization set).
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
shape cache holds on iOS: frust layout (re-shaping) p50 is only
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
~188ms is down to ~85ms** (the warmup optimization set) — the first-frame floor on
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
   anyway).
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
14. **S8 per-op numbers predate shared-script parsing.** As on every other
    device, the S8 per-op medians above were computed outside `stats.py` —
    the shared script has never parsed a per-op `plugin`/`op` line
    (PROTOCOL §7's per-op-line-format methodology-deviations note), only
    `*-perf raw` frame lines and `bench-scenario-*` markers. Reconciling
    S8's per-op parsing into the shared script is the harness task's job.

---

## App size (release) — re-measured post-reset

Host-side snapshot via `benchmarks/harness/app_size.sh`, 2026-07-21, after
the shape-cache/resampling/warmup optimization set (supersedes the retired
pre-reset numbers; Android
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


## Device: OnePlus 9 (LE2115, Snapdragon 888) — mid-tier Android

**Status:** run 2026-07-21 (post-reset re-run; this device's retired pre-reset
section is in git history), **full-form: 12 runs × 30s per scenario per app,
first 2 discarded (10 kept) — PROTOCOL §4 satisfied** (S7 used 60s runs for
the protocol's 60s idle window). **Frust = RELEASE build; Flutter = profile
(per protocol §2). Raw format v2** (`encode_us`+`present_us` split). Raw
series (sanitized to perf/marker lines at capture time by `run.sh`) committed
under `raw/oneplus9/frust_release/<scenario>/run-NN.log` and
`raw/oneplus9/flutter/<scenario>/run-NN.log`, plus per-run
`run-NN.pss_before/after.txt` meminfo snapshots.

- Chipset: Snapdragon 888 / Adreno 660
- Model: OnePlus 9 (LE2115), serial `53f887ac`
- OS: Android 15 (stock OxygenOS)
- Display: 1080×2400 @ 450dpi, two modes (60Hz / 120Hz), **120Hz mode active
  for the whole session** (mode id=2, renderFrameRate 120.00001). Both
  budgets (16.67ms / 8.33ms) therefore reported.
- High-refresh opt-in engaged on both apps (fairness gate §6): Frust
  `Surface.setFrameRate` hint, Flutter `flutter_displaymode`. Verified by
  achieved rates: both apps sustain ~119.5–119.7fps in S4; Flutter ~119.2fps
  (S6) / ~112.2fps (S5). **Achieved-rate caveat:** the effective cadence is
  content/governor-dependent — Frust's S1/S2/S3 settle at a ~60–66fps regime,
  its S5 at ~48fps and S6 at ~104fps, while Flutter's S1 runs ~101.5fps;
  per-run frame counts are in the raw logs. All percentile math is per-frame
  and budget-referenced, unaffected by the attempted rate.
- Fixed brightness 128/255, auto-brightness disabled (`device_state.sh`).
- Airplane mode on (read-back 1), Wi-Fi disabled, Bluetooth disabled.
- Charger: physically USB-connected (adb requirement); `dumpsys battery
  unplug` presented an on-battery state to the DVFS/thermal governor,
  re-applied before every scenario block (run.sh's exit cleanup resets the
  spoof between blocks). `svc power stayon true` during blocks. All state
  restored + read-back confirmed after the session.
- Thermal: gate-based (`device_state.sh`, `dumpsys battery` temperature,
  ceiling 38°C) before each block. Session start 29.1°C, end-of-matrix
  31.6°C, peak 38.2°C (after the S5 flutter block). **One gate stall in 16
  blocks**: the frust S6 block's pre-gate read 38.1°C and the 120s cooldown
  timed out (battery-temp sensor cools slowly with the screen on), the gate
  proceeded per its documented degrade-to-warning — that block's wall time
  505s vs the uniform 382–385s of every other 30s block (743s for both 60s
  S7 blocks). See deviation 8.
- Flutter version: 3.44.2 stable — `flutter build apk --profile`
  (75,819,272-byte profile APK; package id `it.f0x.flutter_bench`, passed
  via `--pkg`). Byte-identical APK to the Xiaomi 12 pass (built the same
  day; no code drift between the two sessions, verified by git).
- Frust build: **RELEASE** (`frust build apk --release`, 22,287,050-byte
  universal APK) — huddle upload keystore, JBR `JAVA_HOME`, NDK
  `27.0.12077973`, `FRUST_TRACE=1 FRUST_TRACE_RAW=1` exported in the build
  shell + `perf.rs` mtime touch (the long-standing release `--define`-gap
  recipe). **Byte-identical APK to the Xiaomi 12 pass** (code state
  `a55e09e`-equivalent: S5 layout-parity v2 + the full
  shape-cache/resampling/warmup optimization set; repo HEAD at run time
  `c92e795`, a docs-only delta). **v2 raw
  emission (`encode_us=`/`present_us=` split fields) confirmed on-device
  before any scored run**; stale prior installs of both apps were
  uninstalled and both APKs installed fresh first.
- Visual gates: every scenario screencap-spot-checked on both apps before
  its capture block (no empty renders anywhere; both S1 fields render the
  identical deterministic bubble layout — same tickers/positions on both
  apps). **S5 mid-scroll parity explicitly verified: frust cells render
  edge-to-edge full width (8px side padding), identical geometry to
  Flutter's — the layout-parity v2 fix holds on this device.**
- Sanitization: after every block and again post-matrix, all raw logs were
  scanned — zero lines outside the `frust-perf|flutter-perf|bench-scenario|
  blank` whitelist in all 192 committed run logs; all 384 PSS snapshots
  reference only the bench package; the `.unfiltered` full-logcat scratch
  was confirmed auto-deleted (none persists).

### S1 — Animation storm (spec v3, settle-capable field)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames (10 kept runs) |
|---|---|---|---|---|---|---|---|
| Frust (release) | 9.70 | **12.19** | **15.49** | 56.49 | **149 (0.77%)** | 14,917 (77.1%) | 19,351 (~64.5 fps avg) |
| Flutter (profile) | **9.23** | 16.90 | 19.08 | **38.61** | 1,553 (5.1%) | 26,987 (88.7%) | 30,438 (~101.5 fps avg) |

**Split result — reproduces the retired 2026-07-20 verdict almost exactly**
(retired: frust 9.75/12.41/15.79, flutter 9.27/16.87/19.25). Frust wins
tail/jank consistency — p95/p99 lower, and it drops the 60Hz budget ~6.6×
less often per frame attempted (0.77% vs 5.1%) — while Flutter wins the
median by a hair and the raw frame rate (~101.5 vs ~64.5 fps). The Xiaomi
12's clean frust S1 win does **not** transfer to Adreno 660: vello's
advantage there rode a ~120fps locked cadence this GPU doesn't reach at this
workload. Neither app recorded a single `skipped=1` frame (0/19,351;
0/30,438) — the field never settled within any 30s window, the same genuine
finding as both prior passes.

### S2 — Long-list scroll (10k rows)

Health-gated: frust `layout_us > 0` on 18,037/18,047 kept frames (p50
3,770µs) — a genuine per-frame scroll relayout.

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | **10.87** | **14.97** | **17.96** | 56.12 | **296 (1.6%)** | 16,955 | 18,047 (~60 fps continuous) |
| Flutter (profile) | 14.57 | 23.97 | 27.09 | **44.59** | 1,955 (31.5%) | 5,020 | 6,200 (event-driven, ~620/run) |

**Frust wins S2** — median 10.9 vs 14.6ms, p95 15.0 vs 24.0ms, and a ~20×
lower 60Hz-miss rate (1.6% vs 31.5% of frames attempted). Reproduces both
the retired series (11.04/15.52) and the shape-cache-era re-run (10.87/14.98 —
p50 identical to the digit). Flutter's lower @8.33ms absolute count reflects
its ~3× smaller frame total (event-driven paint), not better per-frame cost.

### S3 — Table ops (js-framework-benchmark subset, continuous cycling)

Cycle health confirmed on both apps (kept-run `bench-scenario-start
s3-create1k` reopen counts: frust 20–21/run, flutter 19/run — continuous
cycling, non-empty table between the intentional settle gaps).

**Per-op reconcile-frame timing** (`s3-*` sub-markers aggregated across all
cycles in the 10 kept runs):

| Op | Frust p50/p95/p99/worst (ms, release) | n frames | Flutter |
|---|---|---|---|
| create 1k | 11.31 / 16.50 / 52.30 / 54.34 | 204 | **not captured** |
| create 10k | 22.52 / 26.39 / 27.14 / 27.48 | 201 | **not captured** |
| update every 10th of 10k | 16.96 / 19.79 / 20.69 / 22.13 | 199 | **not captured** |
| swap | 16.05 / 18.64 / 19.33 / 19.97 | 195 | **not captured** |
| clear | 17.50 / 20.29 / 21.68 / 22.66 | 194 | **not captured** |

**Flutter per-op capture landed zero frames in every sub-marker window**
across all 10 kept runs — the same structural
`SchedulerBinding.addTimingsCallback` async-delivery gap documented on all
three prior device passes (fourth device in a row); the `flutter_bench`
marker-placement fix remains the recorded follow-up. No cross-app per-op
comparison exists on this device.

Frust's per-op medians all improved vs its retired pre-optimization numbers
(create1k 12.48→11.31, create10k 24.02→22.52, update 19.31→16.96, swap
17.81→16.05, clear 17.91→17.50ms); create10k's 22.5ms is the only per-op
median outside a single 60Hz frame, and create1k keeps its first-occurrence
tail outlier (p99 52.3ms), as on every device.

**Overall S3 frame series** (whole capture incl. settle gaps — *not*
apples-to-apples, continuous vs event-driven paint): Frust p50 8.99 / p95
12.20 / p99 20.43 / worst 54.34ms (19,782 frames, ~66 fps continuous);
Flutter p50 19.32 / p95 34.90 / p99 35.94 / worst 81.30ms (919 frames,
~3.1 fps event-driven — paints only at op transitions). No overall winner
declared from these, per the established convention.

### S4 — Heavy-work responsiveness (~50MB JSON parse + concurrent animation)

| Metric | Frust (release) | Flutter (profile) |
|---|---|---|
| Total wall time (parse complete) | not captured (deviation 2) | not captured |
| Animation p50 during parse (ms) | 4.64 | **4.23** |
| Animation p95 during parse (ms) | 5.50 | **5.01** |
| Animation p99 during parse (ms) | 6.11 | **5.79** |
| worst (ms) | 57.61 | **14.94** |
| Missed-budget during parse (@16.67 / @8.33) | 20 / 104 | **0 / 16** |
| Active frames (10 kept runs) | 35,921 (~119.7 fps) | 35,848 (~119.5 fps) |

**Roughly a tie, Flutter marginally smoother** — the same verdict as every
prior device. Both apps hold a locked ~120fps animation while the heavy
parse runs off the UI thread (Frust `spawn_blocking`, Flutter
`Isolate.run`); Flutter's tail is much tighter (worst 14.9 vs 57.6ms).
Reproduces the retired pass (4.61/5.42 vs 4.23/4.63) within noise.

### S5 — Image pipeline (decode-and-display while scrolling) — layout-parity v2

**First S5 series on this device under the v2 full-width cell geometry**
(both apps compositing identical edge-to-edge cells — verified visually
on-device, see the metadata bullet). Health-gated: frust `layout_us > 0` on
14,371/14,381 kept frames — genuine scroll. **The retired pre-reset S5 rows
(frust p50 9.85ms) composited ~⅓ fewer pixels and are not comparable.**

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | 15.71 | 20.08 | 20.94 | 56.23 | 6,011 (41.8%) | 13,838 | 14,381 (~47.9 fps) |
| Flutter (profile) | **6.14** | **13.87** | **15.96** | **23.27** | **147 (0.44%)** | 9,600 | 33,671 (~112.2 fps) |

**Flutter wins S5 decisively on this device** — the first unambiguous
Flutter win in this matrix pass, and a much wider gap than the Xiaomi 12's
narrow S5 loss (5.7 vs 5.3ms there). Under the fair full-width geometry the
Adreno 660 pass is dominated by Frust's `present_us` (p50 9.25ms, 58.9% of
the median frame; encode p50 5.23ms) — the swapchain-acquire wait at this
composite size bounds the loop at ~48fps and pushes 41.8% of frames past
the 60Hz budget, while Flutter sustains ~112fps with a 6.1ms median.
Honest read: on this GPU tier the v2 geometry exposes a real Frust
present-path bottleneck in S5 that the old undersized cell masked.

### S6 — Text shaping stress (multilingual relayout on width animation)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | **5.67** | 10.75 | 12.14 | 58.14 | 22 | **4,490** | 31,115 (~103.7 fps) |
| Flutter (profile) | 8.35 | **9.75** | **10.19** | **11.93** | **0** | 18,032 | 35,747 (~119.2 fps) |

**Split — Frust wins the median (by 32%), Flutter wins the tail.** Same
shape as the Xiaomi 12 verdict. The shape cache holds exactly on
this device: frust p50/p95 5.67/10.75ms vs the shape-cache-era re-run's 5.67/10.86
(and 10.66/14.52 retired pre-cache — a 47% median improvement carried
forward). Flutter's distribution is tighter (p95 9.75 vs 10.75ms, worst
11.9 vs 58.1ms, zero dropped 60Hz frames) at ~119fps vs Frust's ~104fps;
frust misses the 120Hz budget on far fewer frames (4,490 vs 18,032).

### S7 — Cold start + idle (60s runs, protocol-compliant idle window)

Idle memory = mean TOTAL PSS over the 10 kept runs' post-run
`dumpsys meminfo` snapshots (committed alongside the logs). Idle CPU
sampled via `dumpsys cpuinfo` every 30s across both blocks (54 samples).

| Metric | Frust (release) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of 3 kept of 4) | **~185 ms** (180/185/196) | ~408 ms (397/408/418) |
| Framework-reported first-frame span | ~128 ms median (`first_frame_presented`, 121–150 across 12 launches) | **~76 ms** median (`first_frame_ms`, 6–82) |
| Idle CPU, 60s window (avg %) | ~0% (0% in every sample) | ~0% (0% in every sample) |
| Idle memory (PSS, mean MB, 10 kept runs) | **~104.5 MB** (104.2–104.7) | ~150.5 MB (150.3–150.7) |

**Frust wins S7.** External identical-tool cold start ~2.2× faster (185 vs
408ms — improved from the retired 215ms; Flutter unchanged ~417→408ms) and
idle memory ~31% lower (104.5 vs 150.5 MB PSS; both apps' PSS rose vs the
retired pass — 98.1→104.5 and 125.7→150.5 — same direction on both sides,
and the run-to-run spread is now <1 MB on both, vs Flutter's 88–149 MB
variance in the retired pass). The framework-self-reported first-frame
still favors Flutter (76 vs 128ms) as on every device; Frust's
`first_frame_presented` median improved 147→131.5→128ms across the three
passes (startup optimization work carried forward; GPU init `adapter_ready`
≈ 55ms is the floor on this Adreno 660). Both apps idle at 0% CPU in every
sample (frame gate / event-driven idle; frust's instrumented build emits
~71 skipped-frame trace lines/s while idle — a trace-build artifact, not
real work, and CPU still reads 0%).

### S8 — Plugin-call overhead (shared preferences)

Per-op median latency over the 10 kept runs (µs/call; first call per type
excluded as warmup; 1,990 samples per op per type per app). Frust
`bool`/`i64`/`f64`/`String`/`Vec<String>` ↔ Flutter
`bool`/`int`/`double`/`String`/`List<String>`. **Zero errors observed**: no
`s8-errors` marker in any of the 24 logs (kept or discarded), both apps —
every write `Result` ok, every read verified against its expected value.

| Op | Frust (µs/call, release) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | **102** | 290 | n/a |
| write i64 | **99** | 286 | n/a |
| write f64 | **67** | 307 | n/a |
| write String | **65** | 278 | n/a |
| write Vec\<String\> | **67** | 288 | n/a |
| read (unique key, forces channel) bool | **17** | 4367† | ~0 |
| read (unique key, forces channel) i64 | **17** | 4367† | ~0 |
| read (unique key, forces channel) f64 | **18** | 4367† | ~0 |
| read (unique key, forces channel) String | **18** | 4367† | ~0 |
| read (unique key, forces channel) Vec\<String\> | **18** | 4367† | ~0 |

**Frust wins S8 decisively — the clearest, most robust Rust-advantage
result on its fourth consecutive device pass** (~2.8–4.4× on writes, the
boundary-crossing headline). The per-type medians reproduce this device's
retired `bfd6fd4` revalidation to within 1–4µs (writes 106/102/69/67/69 →
102/99/67/65/67; reads 18 → 17–18µs). †Flutter's only channel-crossing
read is the package's `reload()` whole-store re-read (median 4,367µs, one
figure for all read rows; retired: 4,372µs); Frust's read is a real per-key
backend call at ~17–18µs. Flutter's cached read (~0µs) is a Dart-map
lookup, not a boundary crossing. The burst-during-animation variant was not
run this pass (deviation 3).

### Methodology deviations (this device)

1. **Flutter release-mode in-app cross-check (PROTOCOL §2) not captured** —
   only the profile-mode headline series was taken (matches all prior
   passes).
2. **S4 parse wall-time not recoverable** — the capture is timestamp-free
   (`logcat -v raw`) and neither app emits a parse-duration line; only the
   animation-during-parse percentiles are reported (same as prior devices).
3. **S8 burst-during-animation variant not run** this pass (matches all
   prior passes).
4. **S3 Flutter per-op capture landed zero frames** in every sub-marker
   window — structural `addTimingsCallback` async-delivery gap, fourth
   device in a row; `flutter_bench` marker-placement fix remains the
   recorded follow-up.
5. **External cold start** measured post-matrix (4 launches/app, first
   discarded, 3s force-stop gap) rather than inside the run.sh loop; S7's
   in-loop launches provided the 12 framework-span samples per app.
6. **Charger:** physically USB-connected throughout (adb); `dumpsys battery
   unplug` spoofed on-battery state identically for both apps (re-applied
   per block). Battery 98%→100% real charge over the session under the
   spoof.
7. **Frust release trace-injection quirk** (known recipe): release
   `--define` does not reach the cargo compile; `FRUST_TRACE=1
   FRUST_TRACE_RAW=1` exported in the build shell + `perf.rs` mtime touch.
   This pass reused the byte-identical APK built with that recipe for the
   same-day Xiaomi 12 pass (no code drift, verified); v2 emission
   re-confirmed on this device before scoring.
8. **One thermal-gate stall in 16 blocks:** the frust S6 block's pre-gate
   temperature read 38.1°C (0.1°C over ceiling) and the gate's 120s
   cooldown timed out without dropping below 38°C (screen-on battery-temp
   sensor cools slowly), proceeding per the gate's documented
   degrade-to-warning; block wall time 505s vs the uniform 382–385s
   elsewhere. All other 15 blocks passed the gate immediately (session peak
   38.2°C after the S5 flutter block; start 29.1°C, end 31.6°C).
9. **Achieved-rate observation** (not a capture defect): frust runs S1 at
   ~64.5fps, S2 ~60fps, S3 ~66fps, S5 ~48fps, S6 ~104fps; Flutter's S1 runs
   ~101.5fps and S5 ~112fps despite the 120Hz panel mode. Percentile math
   is per-frame and budget-referenced.
10. **S8 per-op numbers predate shared-script parsing.** As on every
    other device, the S8 per-op medians above were computed outside
    `stats.py` — the shared script has never parsed a per-op `plugin`/`op`
    line (PROTOCOL §7's per-op-line-format methodology-deviations note),
    only `*-perf raw` frame lines and `bench-scenario-*` markers.
    Reconciling S8's per-op parsing into the shared script is the harness
    task's job.

### Cross-pass notes (vs this device's retired pre-reset series)

- **Reproduction quality is excellent** where the workload didn't change:
  S1/S2/S4 frame percentiles, S3 frust per-op, S7 idle CPU and S8 latencies
  all land within noise (or the expected optimization improvement) of the
  retired numbers — S2's frust p50 matches the shape-cache-era re-run to the
  digit (10.87ms).
- **Optimization gains carried forward:** S6 median 10.66→5.67ms (shape cache),
  S3 per-op medians all lower, external cold start 215→185ms,
  `first_frame_presented` 147→128ms.
- **S5 is the one verdict flip, and it is honest:** the retired split
  (frust p50 9.85ms at ~66% cell width) becomes a decisive Flutter win under
  the v2 full-width parity geometry — frust's present-path wait dominates at
  this composite size on Adreno 660 (`present_us` p50 9.25ms), a real
  finding the undersized cell had masked. The Xiaomi 12 (Adreno 730) shows
  the same direction at much smaller magnitude (5.7 vs 5.3ms).

### Release-vs-profile comparability bound — A/B (2026-07-23)

**Purpose:** bound the delta introduced by the release-lean methodology break
(PROTOCOL §2.5): Frust perf runs moved `--release` → `--profile` because
release now compiles instrumentation out entirely.

**Setup / deviations (labeled):** single session, this device (serial
`53f887ac`), 120Hz mode. Profile APK = `frust build apk --profile --define
FRUST_TRACE_RAW=1` (25,555,162 B; `frust.cargoFeatures` → `frust/perf-trace`
via the release-lean gradle plumbing, hand-synced into frust_bench this
session — it previously sat outside the profile-sync tripwire's coverage
(root, template, and `examples/huddle` only)). Release APK =
`frust build apk --release` **with both trace defines deliberately passed**
(24,926,234 B). Airplane on, `dumpsys battery unplug` spoof (USB attached
for adb, as the 2026-07-21 pass), brightness 128, thermal gate ≤38°C
(session range 25.1–31.9°C). **Deviation:** 12 runs in ONE block per
scenario (no multi-day repetition); raw logs committed under
`raw/oneplus9/frust_profile_task09/` and `frust_release_task09_oneoff/`.
**Code-version caveat:** the 2026-07-21 release baseline predates the
render-thread split, shader pre-pass, glyph design language, and
release-lean work that today's HEAD adds — the cross-methodology rows
below are therefore ALSO cross-code-version and labeled as such.

**Release genuinely cannot emit (sanity arm):** one 30s run of S1 and S3
each on the release APK captured ZERO Rust-side perf lines despite both
defines being passed at build time (each log holds only the app-template
Kotlin `activity-create` marker). Release-lean verified at the emission
level on the shipped device artifact; the `.so` strings check also passed
on this session's artifact (0 frust-perf / 0 bench-scenario, 21
`frust-render:` warn strings retained — `release-lean-check.sh --android`).

**Frame stats (same stats.py, 10-kept-of-12 convention both sides):**

| Scenario | Series | p50 (ms) | p95 | p99 | worst | missed @16.67ms | active frames |
|---|---|---|---|---|---|---|---|
| S1 | release 2026-07-21 (pre-render-split code, v2) | 9.70 | 12.19 | 15.49 | 56.49 | 149 (0.77%) | 19,351 |
| S1 | **profile 2026-07-23 (current HEAD, v3)** | 14.79 | 15.87 | 16.51 | 63.91 | 162 (0.79%) | 20,445 |
| S3 | release 2026-07-21 (pre-render-split code, v2) | 8.99 | 12.20 | 20.43 | 54.34 | 546 (2.76%) | 19,782 |
| S3 | **profile 2026-07-23 (current HEAD, v3)** | 10.25 | 12.53 | 20.88 | 52.65 | 957 (3.10%) | 30,880 |

**Presented-rate mode A/B (same HEAD, same conditions, HUD presented-fps):**
release S1 **121.0 fps** vs profile S1 **121.0 fps** — 0% delta at the
user-visible presented-frame level (screencap-verified both).

**Verdict / bound:** TAILS are comparable within noise across the
methodology break — p95 +2.7% (S3) / p99 +2.2% (S3) and p99 +6.6% (S1),
with @16.67ms miss RATES near-identical (0.77→0.79% S1) — and the same-HEAD
presented-fps A/B shows 0% mode delta. MEDIANS are NOT comparable
(+52% S1, +14% S3): per the >5% rule this is flagged — but the S1/S3
median shift cannot be attributed to profile-mode overhead because the code
version moved too (the render-thread split's known pacing change; S3 processes +56%
more frames in the same window on current HEAD). PROTOCOL §2.5's break note
therefore stands as: **tail/jank metrics and miss rates carry across the
break within ~3–7%; medians and raw-cadence comparisons against
pre-2026-07-23 rows are invalid without a same-code release reference,
which release-lean makes impossible by design — use the HUD presented-fps
figure (parity) for mode-only questions.**

---

## Device: Pixel 5a (barbet, Snapdragon 765G / Adreno 620) — mid-tier Android

**Status:** matrix run to completion on the Pixel 5 stand-in (area × 1.0/0.75/0.5, 3 kept of 5 runs per scenario), msaa cells excluded as corrupted, Decision recorded, raw series committed. **Row 1 is the pre-existing 2026-08-27 diagnosis baseline** (`area` / `1.0`, S1/S2 only, device-state gate skipped). **Rows 2–4 are the Pixel 5 stand-in `ab_matrix.sh` run** (2026-08-28, `--skip-device-state`, 5 runs × 20 s captured, 3 kept, 60Hz mode, USB power on).

- Chipset: Snapdragon 765G / Adreno 620
- Model: Google Pixel 5a (barbet), serial `17281JECB01994`
- OS: Android 14, build AP2A.240805.005
- Display: 1080×2400 @ 420dpi, **60Hz only** — no high-refresh mode, so
  only the 16.67ms budget applies on this device (no 8.33ms column).
- Pixel 5 stand-in: Google Pixel 5 (redfin, serial 13261FDD40030W, Snapdragon 765G / Adreno 620, 1080×2340, Android 14) stands in for the Pixel 5a in the rows below — same SoC and GPU as the Pixel 5a, 2.75× density.

### Fine-floor A/B (remedy 3, plan fplan_000001a03fce41148yUkciag)

Matrix driven by `benchmarks/harness/ab_matrix.sh` (`FRUST_AA_MODE` ×
`FRUST_RENDER_SCALE`, `benchmarks/frust_bench`'s S1/S2/S4 plus
`examples/material3-demo`'s push/pop nav column) — column order matches the
script's own emitted table. Only a cell that has actually been run is
listed; an unrun cell is omitted here, never filled with a placeholder
number (this file's own top-of-document rule, restated in the
methodology-deviations note below).

| AA mode | Render scale | S1 p50 (ms) | S1 p95 (ms) | S2 p50 (ms) | S2 p95 (ms) | S4 p50 (ms) | S4 p95 (ms) | nav total_p50 (ms) | nav submit_p95 (ms) |
|---|---|---|---|---|---|---|---|---|---|
| area | 1.0 | 47.9 | 49.6 | 44.0 | 45.8 | not captured | not captured | not captured | not captured |
| area | 1.0 | 48.23 | 49.13 | 42.23 | 43.98 | 16.71 | 17.45 | 11 | 36 |
| area | 0.75 | 31.88 | 33.04 | 28.05 | 29.45 | 7.53 | 8.19 | 37 | 43 |
| area | 0.5 | 19.23 | 20.18 | 17.87 | 19.82 | 7.83 | 8.31 | 19 | 24 |

Raw series: benchmarks/raw/pixel5/fine-floor/<aa>-<scale>/<scenario>/run-NN.log (+ stats.txt; nav: .../nav/logcat-frust-perf.log), sanitized by run.sh/ab_matrix.sh at capture time. (the baseline row has no committed series — see deviation 5)

msaa8 / msaa16 cells are excluded — both render corrupted on Adreno 620 (device smoke 2026-08-28, black first frames + torn glyph/edge strips; vello 0.9's MSAA fine path; research artifact rsa_000001a0486660f3RiPPR50x; LIMITATIONS.md entry above), so their fine-stage timings would measure a broken pipeline.

**Decision:** Render scale is a real lever and the fine cost is ~80 % pixel-proportional on Adreno 620: S1 fits t ≈ 9.6 + 38.7·s² ms (0.75 → predicted 31.3, measured 31.9; 0.5 → predicted 19.2, measured 19.2), ~10 ms fixed + ~39 ms per full-resolution fine sweep; S4 halves at 0.75 then flattens (7.5 → 7.8); the nav column shows the snapshot cache beating any scale (1.0 with cache 11 ms vs 0.75 inline 37 ms), so a reduced-scale mode must keep the cache. No default changes in Phase 3: `FRUST_RENDER_SCALE` stays a measurement instrument — shipping a reduced scale would need resolution-aware text (bilinear upscale softens glyphs) and a compositor-aware snapshot cache (refused while scaled); the number that decides a later device-tier render-policy plan is the ~38 ms/sweep slope, which only a lower-cost fine stage (e.g. the vello_hybrid spike, if run) or fewer full-surface sweeps per frame can move.

#### Methodology deviations (Fine-floor A/B, this device)

1. **Quick pass, not the PROTOCOL §4 convention:** 5 runs × 20 s per scenario for S1/S2/S4 columns,
   first 2 discarded per PROTOCOL §4 → percentiles over 3 kept runs (vs §4's
   ≥10 runs × 30 s).
2. **No thermal/brightness/airplane-mode/charger gate:** every scenario
   batch runs with `run.sh --skip-device-state`, so `device_state.sh`'s
   fairness gate (PROTOCOL §3) never runs for this matrix — environmental
   controls are uncontrolled for every cell, baseline row included.
3. **Baseline row predates the script:** the `area` / `1.0` row's S1/S2
   numbers come from the 2026-08-27 diagnosis session, not an
   `ab_matrix.sh` invocation — recorded here as the matrix's starting
   point, not reproduced by the script itself.
4. **Rows 2–4 come from a Pixel 5 stand-in, `--skip-device-state`, 60 Hz mode, USB power on; cell area/0.5 was re-run standalone after the first pass was interrupted during its nav step.**
5. **The 2026-08-27 baseline row has no committed raw series** (diagnosis session, perfetto-only quick pass).
6. **Nav column baseline — single passes, not multi-run:** Each nav cell represents one push/pop-navigation pass per cell via `ab_matrix.sh run_nav`, reporting the app's in-process rolling percentiles from the last `frust-perf frame` summary, with no warm-up discard, over n = 120 (area/1.0), 56 (area/0.75) and 102 (area/0.5) frames as recorded in `benchmarks/raw/pixel5/fine-floor/<cell>/nav/logcat-frust-perf.log`. The Decision's cache-beats-scale statement (1.0 with cache 11 ms vs 0.75 inline 37 ms) rests on these single passes, an effect far outside single-session noise and corroborated by the perfetto gate phase-2b-cleanup-device-gate-pixel5.

### Pixel 5 — classic baseline for the engine plan

The number every later engine-plan render/GPU threshold (frust-engine +
frust-gpu plan, phase 0) is judged against. **Adopts** the Fine-floor A/B
`area` / `1.0` row above as the S1/S2/S4 baseline verbatim — cited, not
re-measured, same raw series
(`benchmarks/raw/pixel5/fine-floor/area-1.0/s{1,2,4}/`) — and **completes**
the set with a fresh S5 and S6 pass at the identical `area`/`1.0` settings
and quick-pass run convention, one PROTOCOL §4-compliant S1 anchor
(≥10 runs × 30 s, first 2 discarded) so the quick-pass rows below carry a
calibrated error bar, and a freshly measured material3-demo nav push/pop
pass. `encode+submit p50` and `Graphics (MB)` are derived
columns beyond `stats.py`'s own `format_table` output — see deviation 5
below for how they're computed; every other column reproduces exactly via
`python3 benchmarks/harness/stats.py --scenario <sN> <raw-dir>/run-*.log`.

Both binaries are profile builds with live instrumentation (the release-lean
convention), at the same `area`/`1.0` render settings:

```
# scenario rows (S5, S6, S1 anchor) — built from benchmarks/frust_bench/
frust build apk --profile --define FRUST_TRACE_RAW=1
# nav row — built from examples/material3-demo/
frust build apk --profile --define FRUST_TRACE_RAW=1 \
    --define FRUST_AA_MODE=area --define FRUST_RENDER_SCALE=1.0
```

`FRUST_AA_MODE=area` and `FRUST_RENDER_SCALE=1.0` are the documented
defaults (byte-identical to an untouched build), so the two command lines
describe the same render configuration; naming them on the nav line only
records which settings the pass was driven at. That configuration is
verified on the installed binaries rather than assumed from the command
lines: both APKs log `frust-render aa-mode=area` and `frust-render
render-scale=1 blit-target=1080x2340` once at startup, with no
non-default-knob warning. `FRUST_TRACE_RAW=1` is likewise evident in the
captures themselves — the per-frame `frust-perf raw` lines every number
below is computed from exist only under it.

| Scenario | Basis | Kept runs × duration | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | encode+submit p50 (ms) | Graphics avg (MB) |
|---|---|---|---|---|---|---|---|---|---|---|
| S1 | adopted (Fine-floor A/B `area`/`1.0` row) | 3 of 5 × 20s | 48.23 | 49.13 | 52.02 | 137.44 | 1,235 (100%) | 1,235 (100%) | 47.39 | not captured (see deviation 5) |
| S2 | adopted (Fine-floor A/B `area`/`1.0` row) | 3 of 5 × 20s | 42.23 | 43.98 | 45.81 | 137.51 | 1,484 (100%) | 1,484 (100%) | 39.59 | not captured (see deviation 5) |
| S4 | adopted (Fine-floor A/B `area`/`1.0` row) | 3 of 5 × 20s | 16.71 | 17.45 | 18.55 | 83.40 | 1,928 (54.0%) | 3,570 (100%) | 16.26 | not captured (see deviation 5) |
| S5 | measured, this section (2026-08-28) | 3 of 5 × 20s | **94.71** | 98.52 | 99.64 | 101.01 | 678 (100%) | 678 (100%) | 93.94 | 54.97 |
| S6 | measured, this section (2026-08-28) | 3 of 5 × 20s | 22.44 | 23.36 | 25.70 | 58.10 | 2,705 (99.9%) | 2,707 (100%) | 21.75 | 52.82 |
| S1 anchor | measured, this section (2026-08-28), PROTOCOL §4-compliant | 10 of 12 × 30s | 48.51 | 49.56 | 50.88 | 93.03 | 6,206 (100%) | 6,206 (100%) | 47.71 | 52.56 |

**Nav — material3-demo push/pop (measured, 2026-08-28).** One push/pop pass
driven through `ab_matrix.sh`'s `run_nav` recipe (`am force-stop`, `logcat
-c`, `monkey` launch, 1.6 s settle, three `input tap` / `KEYCODE_BACK` pairs
1.6 s apart at 540,472 / 540,734 / 540,996, then one `adb logcat -d -v raw |
grep -a 'frust-perf'` pipe), snapshot-layer cache left at its default (ON —
the same cache state as the adopted row):

| Nav basis | n (frames) | total_p50 (ms) | submit_p95 (ms) | acquire_p95 (ms) |
|---|---|---|---|---|
| measured — last-120 ring window (the window a summary line covers) | 120 of 141 | **7.39** | 35.95 | 10.89 |
| measured — whole pass | 141 | 7.43 | 57.29 | 10.89 |
| adopted (Fine-floor A/B `area`/`1.0` row), cited, cache on | 120 | 11 | 36 | 12 |

Truncated to whole milliseconds the way `FrameStats::emit_log` prints them,
the measured ring-window figures read `total_p50_ms=7 submit_p95_ms=35
acquire_p95_ms=10` against the cited row's `11`/`36`/`12`. The measured pass
emitted no `frust-perf frame` summary line, so its three figures are
computed from its own committed `frust-perf raw` frame lines instead of read
from a summary — a substitution calibrated against the cited row's raw log,
where the identical computation reproduces that row's emitted summary line
exactly (11.39 → `total_p50_ms=11`, 36.66 → `submit_p95_ms=36`, 12.45 →
`acquire_p95_ms=12`, over the same last-120 window). Deviation 8 below has
the mechanism and the calibration in full.

The measured median lands ~4 ms below the cited one at nominally identical
settings. Refresh mode does not explain it: both logs' own `frust-perf
platform-view tail` lines report a ~11.13 ms display frame period (90 Hz)
during capture. Each figure is a single push/pop pass with no warm-up
discard and no repeat (this file's Fine-floor deviation 6 above states that
convention for the nav column), so a gap of this size between two sessions
is recorded here rather than explained; the whole-pass `submit_p95` column
shows how much the same pass's tail moves with the window alone (35.95 ms
over the ring window, 57.29 ms over all 141 frames).

Raw series: `benchmarks/raw/pixel5/classic-baseline/{s5,s6,s1}/run-NN.log`
(+ `stats.txt` + `run-NN.pss_before.txt`/`run-NN.pss_after.txt`), sanitized
by `run.sh` at capture time — the S1 row there is the protocol-compliant
anchor (12 runs), not the adopted sub-protocol S1 — plus the nav pass at
`benchmarks/raw/pixel5/classic-baseline/nav/logcat-frust-perf.log`,
sanitized in the single `logcat | grep` pipe above (no unfiltered dump is
written anywhere). The adopted S1/S2/S4 and cited-nav rows' own raw series
remains at `benchmarks/raw/pixel5/fine-floor/area-1.0/` (unchanged, not
duplicated).

**Anchor calibration:** the protocol-compliant S1 anchor (48.51/49.56 ms
p50/p95 over 6,206 kept frames, 10×30s runs) lands within 0.3–0.4ms of the
adopted quick-pass S1 row (48.23/49.13 ms, 3×20s) — on this device/scenario
the quick-pass convention is not meaningfully biased relative to the full
PROTOCOL §4 convention, so the S2/S4/S5/S6 quick-pass rows above can be
read with roughly that same error bar.

#### Methodology deviations (classic baseline, this device)

**Device:** Pixel 5 (`redfin`, serial `13261FDD40030W`), Adreno 620,
1080x2340, Android 14, attached over USB. Display mode observed during every
capture in this section: `DisplayMode{id=1, 1080x2340, refreshRate=90.0}`
with `mActiveRenderFrameRate=90.0` (`dumpsys display`), i.e. a ~11.13 ms
frame period — see deviation 4.

1. **S1/S2/S4 rows are cited, not re-measured** — see the Fine-floor
   A/B section's own deviations 1–2 and 4 above (quick-pass 5×20s/3-kept
   convention, `--skip-device-state`, Pixel 5 stand-in, USB power on); they
   apply unchanged to the cited rows here. The cited nav figure carried
   alongside the measured one in the nav table is from that same row.
2. **S5/S6 use the same quick-pass convention as the adopted rows** (5
   runs × 20 s, first 2 discarded, 3 kept) to stay comparable to them —
   below PROTOCOL §4's ≥10×30s convention. The S1 anchor row exists
   specifically to bound how much that shortcut costs on this device.
3. **Device-state fairness gate ran (not skipped) for S5/S6/S1-anchor and
   the nav pass** — unlike the adopted rows' `--skip-device-state` session:
   fixed brightness=128; thermal OK (~30–32°C for the scenario batches,
   27.5°C for the nav batch, ceiling 38°C); airplane mode OFF and USB
   charging detected, both flagged as warnings by `device_state.sh` and left
   uncontrolled (radio/charger state is check-only, never forced — see that
   script's own header). The nav batch ran its own `device_state.sh` pass
   before capture, on the same terms.
4. **Display refresh mode not locked to 60Hz** — the device ran at its
   default active mode, confirmed 90Hz
   (`dumpsys display`'s `mActiveSfDisplayMode`/`mActiveRenderFrameRate=90.0`)
   during every capture in this section, unlike the adopted Fine-floor A/B
   matrix, which explicitly forced "60 Hz mode" (that section's deviation
   4). For the scenario rows this changes nothing measurable: every measured
   p50 there (22–95ms) already misses even the 90Hz (11.1ms) frame budget by
   2–8×, so the missed@16.67/missed@8.33 counts are unaffected. For the nav
   row it is load-bearing and stated rather than argued away — a nav frame
   at a 7.4ms median sits *inside* the 11.1ms period, so it is vsync-paced,
   and its `acquire_p95` (10.89ms, a blocking swapchain wait) is essentially
   one frame period. Note that the cited nav row's own committed log reports
   the same ~11.13ms period in its `frust-perf platform-view tail` lines
   despite that section's forced-60Hz note, so measured and cited nav are
   refresh-comparable to each other.
5. **`encode+submit p50` and `Graphics (MB)` are derived, not `stats.py`
   `format_table` output.** `encode+submit` sums each kept frame's
   `encode_us`+`submit_us` (loaded via `stats.py`'s own
   `load_run_frames`/`discard_first_runs`, then `stats.py`'s
   `_nearest_rank_percentile` — imported and reused, not reimplemented)
   before the percentile step. `Graphics (MB)` averages the `Graphics`
   Pss(KB) line of each kept run's **post-run** `dumpsys meminfo`
   snapshot (`run.sh`'s own `pss_after` capture) — the pre-run snapshot is
   always empty (the app is force-stopped immediately before each run, so
   there is no process to sample), so this is an absolute post-run figure,
   not a before/after delta. The `pss_before`/`pss_after` text dumps behind
   the S5/S6/S1-anchor Graphics figures are committed alongside their
   `run-NN.log`/`stats.txt` (`benchmarks/.gitignore`'s documented
   `*.pss_*.txt` raw-artifact class, scoped to `it.f0x.*bench`'s own
   process — self-check: no foreign package identifier appears in any of
   them). The adopted S1/S2/S4 rows report Graphics as "not captured"
   instead: that series' own committed raw set
   (`run-NN.log`+`stats.txt` only, per `ab_matrix.sh`'s own committed-set
   convention) never captured a meminfo snapshot at all, and the cited rows
   are not re-measured here.
6. **S5's ~94ms median is a genuine, reproducible present-path stall, not
   a capture artifact** — `submit_us` alone accounts for ~93–94ms of the
   ~95ms median frame (`encode_us`/`acquire_us` both near-zero), consistent
   across all 5 captured runs (per-run mean `total_us` 87.6k–90.0k µs, tight
   spread). This mirrors the OnePlus 9 (Adreno 660)'s own S5 present-path
   finding elsewhere in this file (`present_us` 58.9% of its median frame)
   — the same pathology, far more severe on the older/slower Adreno 620.
7. **Raw-path layout for this section.** Every series measured here lives
   under `benchmarks/raw/pixel5/classic-baseline/<scenario>/` — `s1` (the
   protocol anchor), `s5`, `s6` as `run-NN.log`+`stats.txt`+`pss_*` in the
   established per-run layout, and `nav` as the single sanitized
   `logcat-frust-perf.log`. That tree is a sibling of the Fine-floor A/B
   section's `benchmarks/raw/pixel5/fine-floor/<cell>/<scenario>/`, not a
   copy of it: the cited rows keep pointing at their original series, which
   is neither moved nor duplicated.
8. **The measured nav pass emitted no `frust-perf frame` summary line, so
   its percentiles are computed from that pass's own raw frame lines.**
   `FrameStats::emit_log` is rate-limited to one summary per 2s of
   *accumulated frame time* (`EMIT_INTERVAL` in
   `crates/frust-shell-common/src/perf.rs`), and one three-pair push/pop
   pass renders less than that on this build: the two passes driven
   accumulated 1.92s over 150 frames and 1.85s over 141 frames, so the rate
   limit never fired in either. The second pass is the committed one. Its
   `total_p50`/`submit_p95`/`acquire_p95` are computed from its
   `frust-perf raw` lines with `stats.py`'s own `_nearest_rank_percentile`
   (imported, not reimplemented) over the last 120 frames — `RING_CAPACITY`,
   the same window `FrameStats::summary` reports over. The substitution is
   calibrated rather than assumed: run over the cited nav row's raw log —
   across the window that row's summary actually covered, its last 120
   frames as of its own `total_frames=133` — it reproduces all eleven
   fields of that emitted line exactly (`total_p50/p95/p99`,
   `rebuild/layout/paint/encode/acquire/submit_p95`, `over_60hz`,
   `over_120hz`) once truncated to whole milliseconds as `emit_log` prints
   them. No figure in the nav table is read from a summary line that does
   not exist, and none is estimated.
9. **What is measured here versus cited.** S5, S6, the S1 anchor and the nav
   pass were measured on this device for this section. S1, S2 and S4 are
   cited verbatim from the Fine-floor A/B `area`/`1.0` row above and were
   not re-run (deviation 1); the nav table carries that row's nav figure
   next to the measured one rather than replacing it, so the two bases stay
   visible side by side. `ab_matrix.sh` was read for the nav recipe but
   neither modified nor executed — the nav pass above was driven step by
   step, so no matrix cell, staging directory or published-cell path was
   touched.

## vello_hybrid spike — Pixel 5, iOS Simulator, macOS Metal (engine plan Phase 0)

The GO/NO-GO numbers for the frust-engine plan's Phase 0: the experimental
`vello_hybrid` render tier (`crates/frust-render/src/hybrid_tier.rs`;
`RenderTier::Hybrid` is override-only — no probe ever selects it,
`tier.rs`) measured against the vello-classic rows published above, plus the
two platform questions (does the iOS Simulator render at all, does macOS
Metal run the tier). **No adoption recommendation is made here — only the
numbers, and an explicit statement of what could not be measured.**

Driven by `benchmarks/harness/ab_matrix.sh`'s new `--tier` axis:

```
# hybrid arm (ONE cell — the arm ignores --aa/--scale, see below)
bash benchmarks/harness/ab_matrix.sh --device 13261FDD40030W --device-name pixel5 \
    --tier hybrid --scenarios s1,s2,s4,s5,s6 --runs 5 --duration 20
# glyph-atlas-cache arm
bash benchmarks/harness/ab_matrix.sh --device 13261FDD40030W --device-name pixel5 \
    --tier hybrid_atlas --scenarios s6,s2 --runs 5 --duration 20
```

Each arm builds `benchmarks/frust_bench` and `examples/material3-demo` as

```
# hybrid
frust build apk --profile --define FRUST_TRACE_RAW=1 \
    --features hybrid-tier --define FRUST_RENDER_TIER=hybrid
# hybrid_atlas = the same, plus
    --define FRUST_HYBRID_ATLAS_CACHE=1
```

`FRUST_AA_MODE`/`FRUST_RENDER_SCALE` are deliberately NOT passed on a hybrid
arm — they are vello-classic instruments (`context.rs`'s `parse_aa_mode`
feeds vello's `AaSupport`; the scale knob forces vello's blit arm) and
`vello_hybrid` reads neither, so each hybrid arm is one cell however long
`--aa`/`--scale` are and its AA/scale table cells read `n/a`.

The configuration is verified on the installed binaries rather than assumed
from the command lines: both hybrid APKs log `frust-render tier=hybrid
(experimental vello_hybrid path, format=Rgba8Unorm 1080x2340)` and
`frust-perf render-path hybrid-direct (vello_hybrid into the acquired
swapchain view ...)` once at startup, plus `frust-render hybrid
atlas-cache=off` (hybrid) / `atlas-cache=on` + `frust-render measurement knob
in effect: FRUST_HYBRID_ATLAS_CACHE=1` (hybrid_atlas). The surface is opaque
on this device (`frust-render surface-caps: alpha_modes=[Inherit]
chosen=Auto`), so every hybrid number below is an opaque-surface number —
which costs the arm nothing either way, since the tier itself outputs
PREMULTIPLIED alpha (`vello_hybrid`'s user-surface strip pipelines blend
`PREMULTIPLIED_ALPHA_BLENDING`): it is correct on the premultiplied-expecting
translucent modes (`Inherit`, `PreMultiplied`) and refuses translucency only
on the straight-alpha one (iOS's `PostMultiplied`).

**Rendered output was spot-checked, not assumed.** Device screenshots of the
hybrid build (S1 bubbles with radial-gradient fills and shaped labels — its
own in-app readout showing `FPS: 89.8`; S5's full-width generated images;
S6's Latin/CJK/Arabic/Hebrew/Devanagari paragraphs; material3-demo's home
list) all render correctly, so the numbers below are not a blank or
partially-drawn frame. The screenshots live outside the repo with the run's
other scratch (`benchmarks/raw/` commits no images).

### Arm 1 — Pixel 5 frame scenarios, hybrid vs classic

Same device, protocol convention and settings as the classic-baseline
section above (5 runs x 20 s, first 2 discarded, 3 kept;
`--skip-device-state`), so the classic column is **cited** from the rows
above, not re-measured.

| Scenario | classic p50 / p95 (ms) — cited | hybrid p50 / p95 (ms) | hybrid frames kept | hybrid missed @16.67ms | hybrid encode+submit p50 (ms) |
|---|---|---|---|---|---|
| S1 — animation storm | 48.23 / 49.13 | **12.20** / 13.90 | 5,012 | 61 (1.2%) | 11.17 |
| S2 — 10k-row scroll | 42.23 / 43.98 | **13.10** / 14.06 | 5,253 | 2 (0.0%) | 7.24 |
| S4 — heavy-work responsiveness | 16.71 / 17.45 | **9.36** / 10.27 | 5,244 | 5 (0.1%) | 3.56 |
| S5 — image pipeline | 94.71 / 98.52 | **11.47** / 12.30 | 5,242 | 0 | 5.43 |
| S6 — text shaping stress | 22.44 / 23.36 | **11.59** / 12.38 | 5,252 | 0 | 7.22 |

`Graphics (MB)` is **not captured** for any hybrid cell: `ab_matrix.sh`'s
committed raw set is `run-NN.log` + `stats.txt` only (no `dumpsys meminfo`
snapshot), exactly as for the cited fine-floor rows.

**The S5 row is the "full-screen image" case, named precisely.** S5 is not
one `Command::Image` covering the surface under a 0.5-alpha layer:
`benchmarks/frust_bench/src/scenarios/s5_image.rs` scripts a virtualized
vertical stream of 240 distinct generated 256x256 RGBA images (SplitMix64
seed, decoded through the real `frust::decode_image_async` PNG path), each
cell laid out at `(viewport.width - 2*8dp) x 256dp` with `ImageFit::Cover` —
i.e. several *edge-to-edge* image quads per frame, scrolling on a 10 s-per-leg
scripted timeline, with no translucent layer over them. It is the existing
scenario the classic 94.71 ms figure was measured on, so the two columns are
comparable to each other. The literal one-quad-under-a-0.5-alpha-layer case
(LIMITATIONS.md's `render-snapshot-layer-tradeoffs`: in-vello image-quad
attempt p50 96.9 ms vs the out-of-vello compositor's 1.8 ms) is a Pixel 5a
perfetto measurement of a page-transition snapshot and is **cited, not
re-measured, and not run on the hybrid tier** — see "Not measured" below.

### Arm 2 — the CPU/GPU split (`frust-perf hybrid strip_us=<n> record_us=<n>`)

The hybrid tier emits one line per frame under `perf-trace` + `FRUST_TRACE`
(both on in a `--profile` build): `strip_us` is the CPU half — scene reset,
base-colour ground and the whole shared command walk, i.e. every sparse strip
`vello_hybrid` rasterizes on the CPU that frame — and `record_us` is the
renderer's GPU **command-record** CPU time only. GPU *execution* is not in
either window: on this direct-to-surface arm it lands inside the frame's
`submit_us` (`docs/RENDER_ARCHITECTURE.md`'s render-path A/B caveat), so the
GPU-side remainder is reported as whole-frame-minus-strip rather than as a
GPU timer, which this build has none of.

| Scenario | frame total p50 (ms) | strip_us p50 / p95 (ms) — CPU strip | record_us p50 (ms) | total − strip p50 (ms) — GPU-side remainder | acquire_us p50 (ms) |
|---|---|---|---|---|---|
| S1 | 12.20 | 5.42 / 5.96 | 0.62 | 6.78 | 0.05 |
| S2 | 13.10 | 5.17 / 5.48 | 0.46 | 7.94 | 3.12 |
| S4 | 9.36 | 0.98 / 1.37 | 0.42 | 8.38 | 5.63 |
| S5 | 11.47 | 1.80 / 2.14 | 0.42 | 9.68 | 5.34 |
| S6 | 11.59 | 5.10 / 5.17 | 0.47 | 6.49 | 3.88 |

`acquire_us` is the blocking swapchain wait and is load-bearing for reading
this table: the display ran at **90 Hz** (11.13 ms period, see deviations),
so S2/S4/S5/S6 are partly display-paced — their frame total is an upper bound
on the work, and the remainder column includes that wait. S1 is not
(`acquire_us` p50 0.05 ms): its 12.20 ms is real work.

### Arm 3 — glyph atlas cache on (`FRUST_HYBRID_ATLAS_CACHE=1`)

glifo's experimental glyph atlas cache, priced on the two text-heaviest
scenarios. Same device/protocol/runs; the atlas arm is a separate build and
therefore a separate matrix run.

| Scenario | hybrid p50 / p95 (ms) | hybrid + atlas p50 / p95 (ms) | strip_us p50: off → on (ms) | record_us p50: off → on (ms) |
|---|---|---|---|---|
| S6 — text shaping stress | 11.59 / 12.38 | **9.98** / 10.50 | 5.10 → **1.08** (−79%) | 0.47 → 0.51 |
| S2 — 10k-row scroll | 13.10 / 14.06 | **11.31** / 12.39 | 5.17 → **2.46** (−52%) | 0.46 → 0.90 |

The cache's effect is concentrated exactly where the split says it should be:
the CPU strip half of a text frame drops by four fifths on S6 while
`record_us` is unchanged. The whole-frame win is smaller than the strip win
(1.61 ms on S6, 1.79 ms on S2) because both arms are partly display-paced at
90 Hz (S6 `acquire_us` p50 3.88 ms off → 5.60 ms on: the freed CPU time is
partly reabsorbed by the vsync wait).

### Arm 4 — material3-demo push/pop nav

One push/pop pass per arm (three tap/BACK pairs 1.6 s apart at 540,472 /
540,734 / 540,996 — `ab_matrix.sh`'s `run_nav` recipe), no repeats, nothing
discarded, same basis as the nav rows above.

| Arm | basis | n (frames) | total_p50 (ms) | total_p95 (ms) | submit_p95 (ms) | acquire_p95 (ms) |
|---|---|---|---|---|---|---|
| hybrid | last-120 window | 120 of 136 | **15.22** | 18.41 | 17.66 | 0.11 |
| hybrid + atlas | last-120 window | 120 of 127 | **16.49** | 20.49 | 19.68 | 0.10 |
| classic, cache ON (cited, measured 2026-08-28) | last-120 window | 120 of 141 | 7.39 | — | 35.95 | 10.89 |
| classic, cache ON (cited, fine-floor `area`/`1.0`) | summary line | 120 | 11 | — | 36 | 12 |

**The hybrid nav column is an INLINE (uncached) number and the classic one is
not — they are not the same path.** `renderer.rs` states it outright for this
tier: "the snapshot cache and compositor are GPU-tier machinery this tier
bypasses entirely". So the hybrid arm re-renders each transition frame from
scratch, while both cited classic figures had the snapshot-layer cache ON.
The classic *inline* comparison point this file already publishes is the
fine-floor `area`/`0.75` cell (37 ms) and `area`/`0.5` cell (19 ms), where a
scaled frame disables the cache by design — both at reduced resolution, so
neither is a clean like-for-like against hybrid's full-resolution 15.22 ms.
A classic, scale-1.0, cache-off (`FRUST_NO_SNAPSHOT_LAYERS=1`) nav pass was
**not measured** — see below.

### Arm 5 — iOS Simulator (iPhone 16, iOS 18.6, udid B911C0D8-D2FE-4FEF-84C5-E0C574D1A8A8)

**It renders.** `examples/material3-demo` built and launched with the hybrid
tier (`frust run -d "iPhone 16" --profile --define FRUST_TRACE_RAW=1 --define
FRUST_RENDER_TIER=hybrid --features hybrid-tier`) draws its full home page —
app bar, six list cards with icons, bottom navigation, all text — where
vello classic is black on the Simulator by construction. The Simulator's own
capability report in the same console confirms why: `Missing downlevel flags:
DownlevelFlags(INDIRECT_EXECUTION | BASE_VERTEX | CUBE_ARRAY_TEXTURES |
COMPARISON_SAMPLERS)` — `INDIRECT_EXECUTION` is exactly what vello classic
requires (`docs/DEVELOPMENT.md`'s iOS Simulator note) — and the renderer
still reports `frust-render tier=hybrid (experimental vello_hybrid path,
format=Bgra8Unorm 1179x2556)` with **no wgpu error, no validation error and
no panic** anywhere in the console. Two `xcrun simctl io <udid> screenshot`
captures a few seconds apart differ, so the app is rendering live rather than
showing one stalled frame.

Frame *timings* on the Simulator are **not measured**: that console carried
no `frust-perf raw` and no `frust-perf hybrid` lines at all, so there is no
frame series to compute a percentile from (the Simulator is not a
performance-representative target in any case — PROTOCOL §3).

### Arm 6 — macOS Metal (desktop preview)

Both arms run `examples/material3-demo` through the desktop shell on this
Mac, built from that directory as
`FRUST_TRACE=1 FRUST_TRACE_RAW=1 [FRUST_RENDER_TIER=hybrid] cargo run
--profile profile --features [hybrid-tier,]frust/perf-trace` (the `frust`
CLI refuses `--features` on the desktop-bundle targets, and a plain
`cargo run` would compile out every `frust-perf` line, so the CLI's own
debug/profile feature selection is reproduced by hand).

| Arm | render path | surface | first frame total (ms) | frames 2..n: total p50 / p95 (ms) | n | encode p50 (ms) | strip_us / record_us p50 (ms) | acquire p50 (ms) |
|---|---|---|---|---|---|---|---|---|
| classic | `blit (no Rgba8Unorm)` | Bgra8Unorm 1600x1200 | 50.61 | 8.22 / 10.16 | 42 | 0.57 | n/a | 7.45 |
| hybrid | `hybrid-direct` | Bgra8Unorm 1600x1200 | 8.30 | 8.16 / 16.98 | 48 | 0.01 | 0.59 / 0.17 | 7.28 |

**Both arms sit on the display's frame period, so this table does not
discriminate between them**: `acquire_us` p50 is ~7.3-7.5 ms of the ~8.2 ms
frame on both — consistent with a 120 Hz (8.33 ms) period on the built-in
Liquid Retina XDR display the window sat on, though the active refresh rate
was not read from the system and is inferred from these numbers — and the
non-paced work is sub-millisecond on both sides (classic `encode` 0.57 ms;
hybrid `strip` 0.59 ms + `record` 0.17 ms). The hybrid tier **runs on macOS
Metal without error** — that is the load-bearing result here. One observed
one-off: an earlier cold-shader-cache launch of the same hybrid binary paid
55.21 ms on its first frame (`record_us` 49.16 ms of it, i.e. pipeline
creation); the later launch tabulated above paid 8.30 ms for the same first
frame.

**Window size: 1600x1200 physical (800x600 logical at 2x) — the 5120x2880
target was NOT achieved.** `crates/frust-shell-desktop/src/app_handler.rs`
hard-codes `INITIAL_SIZE = LogicalSize::new(800, 600)` and sets no maximise
attribute, and the shell exposes no size knob, so a bigger window needs
either a source change (outside this card's scope) or window-manager
scripting. Accessibility scripting resized the window to 1300x944 points
(~2600x1888 px) on the built-in display, but every attempt to move it onto
the attached 5120x2880 display made the window vanish from the accessibility
tree in both arms (the process kept rendering), so no 5K surface was ever
configured and none is reported.

### Arm 7 — Browser WebGL2: **NO-GO, see `webgl2_arm.md`**

Not measured, and not measurable in-tree at this SHA. Root `Cargo.toml`
compiles neither wgpu's `gles` nor `webgpu` feature (deliberate trim,
`Cargo.toml:89,92`), no shell targets `wasm32-unknown-unknown`, and
`Cargo.lock` has no `wasm-bindgen-test` entry — so there is no wasm target
and no browser surface to render a scene into. OPEN #1 (engine plan) was
decided **(b)** on 2026-08-29: this plan adds no wgpu feature; the
target-gated `wasm32` `gles`/`webgpu` section belongs to the Web Shell plan
(`fplan_000001a02ee9100bPd4uUpRs`) instead. Full detail, upstream citations
(vello_hybrid's own `wgpu_webgl` example and `vello_sparse_tests`' headless-
Chrome recipe), and the planned Phase 2 desktop stand-in
(`FRUST_ENGINE_DOWNLEVEL=1`) are in `benchmarks/harness/webgl2_arm.md`. No
browser frame-time number is recorded here or there.

### Fit comparison against the classic S1 model

`RESULTS.md`'s fine-floor Decision fits classic S1 as `t ≈ 9.6 + 38.7·s²` ms
on this GPU: ~9.6 ms fixed per full-resolution pass plus ~38.7 ms of
pixel-proportional fine sweep. Against the `area`/`1.0` row (S1 48.23, S2
42.23, S4 16.71):

- **The ~10 ms fixed per-pass term is gone.** Hybrid's *entire* S1 frame is
  12.20 ms, and its GPU-side remainder after the CPU strip half is 6.78 ms —
  below the classic model's fixed term alone, on the scenario the model was
  fitted to. Hybrid draws sparse strips, not a full-surface fine sweep, so
  there is no per-pass surface sweep to pay.
- Ratios at scale 1.0: S1 12.20 vs 48.23 (**3.95x**), S2 13.10 vs 42.23
  (3.22x), S4 9.36 vs 16.71 (1.79x), S5 11.47 vs 94.71 (**8.26x**), S6 11.59
  vs 22.44 (1.94x). S2/S4/S5/S6 are partly 90 Hz-paced (Arm 2's
  `acquire_us` column), so those four ratios are lower bounds on the win.
- **No `t(s)` curve can be fitted for hybrid.** The tier ignores
  `FRUST_RENDER_SCALE` entirely, so only `s = 1.0` exists for it — the
  scale-slope half of the classic model has no hybrid counterpart to compare
  against, and none is invented here.

### What could not be measured

- **A classic scale-1.0, cache-off nav pass** (`FRUST_NO_SNAPSHOT_LAYERS=1`)
  — not measured: it is a fourth APK build beyond this card's arms. Without
  it, hybrid's inline nav (15.22 ms) has no full-resolution classic inline
  number to sit beside; the cached classic figures (7.39 / 11 ms) and the
  reduced-scale inline ones (37 / 19 ms) are what this file has.
- **iOS Simulator frame times** — not measured: no `frust-perf raw` lines in
  that console (renders-or-not was the question, and it renders).
- **macOS at 5120x2880, and the root `PushLayer(alpha<1)` >4096-texture
  question** — not measured: the desktop shell caps the window at its
  hard-coded 800x600 logical (1600x1200 px) and no scripted route enlarged it
  onto the 5K display, so no dimension ever approached `LayersConfig`'s
  4096 `max_texture_size`; no error text exists to report because no such
  frame was ever rendered.
- **A macOS steady-state workload** — not measured: the desktop preview
  renders on demand, so the only reproducible frame series available without
  UI automation is the scripted window-resize sequence tabulated above.
- **`Graphics (MB)` for every hybrid cell** — not captured: `ab_matrix.sh`
  commits `run-NN.log` + `stats.txt` only.
- **The literal full-surface-image-under-a-0.5-alpha-layer case on hybrid**
  — not measured: that figure (96.9 ms in-vello vs 1.8 ms compositor) is a
  Pixel 5a perfetto capture of a page-transition snapshot recorded in
  LIMITATIONS.md, not a `frust_bench` scenario, so there is no hybrid arm of
  it here. S5 (above) is the image-heavy scenario that does exist on both
  tiers.
- **Pixel 5a** — not measured: only the Pixel 5 (`13261FDD40030W`) was
  attached for this pass. Every row above is the Pixel 5.

#### Methodology deviations (vello_hybrid spike)

**Device:** Pixel 5 (`redfin`, serial `13261FDD40030W`), Adreno 620,
1080x2340, Android 14, USB. Display mode during every capture:
`DisplayMode{id=1, 1080x2340, refreshRate=90.0}` with
`mActiveRenderFrameRate=90.0` (~11.13 ms period), brightness 128, thermal
status 0 (NONE), battery 100% on USB power — the same display mode and
brightness the classic-baseline section recorded, so measured and cited rows
are refresh-comparable.

1. **Quick pass, not the PROTOCOL §4 convention:** 5 runs x 20 s per
   scenario, first 2 discarded, 3 kept — chosen to match the rows this
   section is compared against, not §4's >=10 x 30 s. The classic section's
   own S1 anchor bounds that shortcut at 0.3-0.4 ms on this device.
2. **No device-state fairness gate:** every scenario batch ran with
   `--skip-device-state` (PROTOCOL §3 uncontrolled), as the fine-floor rows
   did. The environment was recorded rather than enforced (see **Device**).
3. **The classic column is cited, never re-measured** — S1/S2/S4 from the
   fine-floor `area`/`1.0` row, S5/S6 and the measured nav row from the
   Pixel 5 classic-baseline section above. Their own deviations apply
   unchanged.
4. **The hybrid arm's first nav pass was lost and re-driven.** The device's
   screen dozed off during the matrix run (`svc power stayon usb` did not
   hold), so that pass rendered nothing and `ab_matrix.sh` correctly reported
   `SKIPPED` rather than a number. The committed hybrid nav pass was driven
   by hand afterwards with `run_nav`'s exact recipe, with `svc power stayon
   true` set, and is the one tabulated. The five scenario cells were
   unaffected (1,667-1,752 frames per kept run throughout, ~87 fps).
5. **Nav percentiles are computed from the pass's own raw frame lines, not
   read from a summary,** using the same method for every arm:
   `stats.py`'s `_nearest_rank_percentile` (imported, not reimplemented) over
   the last 120 frames — `RING_CAPACITY`, the window `FrameStats::summary`
   reports over. `FrameStats::emit_log` is rate-limited to one summary per
   2 s of *accumulated* frame time and a three-pair push/pop pass sits right
   at that threshold on this tier (hybrid 2.033 s, atlas 2.048 s), so a
   summary is not guaranteed. Where one did fire, the derivation reproduces
   it exactly: over the atlas arm's own summary window (its last 120 frames
   as of `total_frames=124`) the computation yields
   `total_p50=16.585 / p95=21.612 / p99=31.206 / submit_p95=20.586 /
   acquire_p95=0.096`, i.e. the emitted `total_p50_ms=16 total_p95_ms=21
   total_p99_ms=31 submit_p95_ms=20 acquire_p95_ms=0` once truncated the way
   `emit_log` prints. (Its `over_60hz`/`over_120hz` fields are lifetime
   counters, not window counts, and are 1 and 3 higher than the window's own
   57/111 — the only two fields that do not reproduce, and by construction.)
   The hybrid arm's hand-driven pass emitted a summary too
   (`total_p50_ms=15 submit_p95_ms=17`), matching its derived 15.218 / 17.655.
6. **`encode+submit p50` is derived, not `stats.py` `format_table` output** —
   same computation as the classic-baseline section's column of that name
   (each kept frame's `encode_us`+`submit_us` summed, then
   `stats.py`'s own percentile). The `strip_us`/`record_us` columns are
   derived the same way from the committed `frust-perf hybrid` lines, sliced
   to each scenario's `bench-scenario-start/end` bracket exactly as
   `stats.py` slices frames and with the identical first-2-runs discard.
   `stats.py` itself needed no change: its parser matches the `frust-perf
   raw` prefix and ignores every other `frust-perf` line, so the new hybrid
   line rides the committed logs without disturbing any published row.
7. **The two hybrid arms are separate builds and separate matrix runs**, so
   the atlas comparison carries one build/install/thermal cycle of
   between-run variation on top of the usual single-session noise.
8. **macOS numbers come from a scripted window-resize sequence, not a
   benchmark scenario** (six `set size of window 1` steps 1 s apart, identical
   for both arms) — the only reproducible way found to make the on-demand
   desktop shell render a frame series. They are single passes with no
   warm-up discard, and both arms are display-paced (see Arm 6).
9. **iOS Simulator evidence is log- and screenshot-based**, on a Simulator
   rather than a device: a rendering-or-not answer, not a performance one.

**Raw series:** `benchmarks/raw/pixel5/tier/hybrid/{s1,s2,s4,s5,s6}/` and
`benchmarks/raw/pixel5/tier/hybrid_atlas/{s2,s6}/` as
`run-NN.log` + `stats.txt`, sanitized by `run.sh`/`ab_matrix.sh` at capture
time; the nav passes as
`benchmarks/raw/pixel5/tier/{hybrid,hybrid_atlas}/nav/logcat-frust-perf.log`,
sanitized in the single `adb logcat -d -v raw | grep -a 'frust-perf'` pipe
(no unfiltered dump is written anywhere). Every scenario row above
reproduces via `python3 benchmarks/harness/stats.py --scenario <sN>
benchmarks/raw/pixel5/tier/<arm>/<sN>/run-*.log`. The cited classic rows keep
pointing at their original series under
`benchmarks/raw/pixel5/{fine-floor,classic-baseline}/`, neither moved nor
duplicated. The macOS and iOS Simulator arms have no committed series: their
consoles are host-side logs, not device captures in the harness's sanitized
form.

---

## DB scenarios (`d1`/`d2`) — no runs recorded yet

`PROTOCOL.md` §9 specifies the `d*` scenario class: op-latency DB
benchmarks distinct from the frame-class `s1..s8` table above — `d1`
(batched-transaction + autocommit writes) and `d2` (point SELECT + range
scan), both against the fixed row shape and deterministic seed dataset
in PROTOCOL §9.2, and both speaking §7's per-op raw-line contract.

**No device has run this matrix pass yet.** Per this file's own
discipline (see the top of this file): a scenario or device with no
completed runs is omitted here, not filled with placeholder numbers — so
this section carries no table and no figures. Once a `d*` pass is
captured, this section gains one subsection per device (mirroring the
S1–S8 device blocks above), each carrying:

- Three columns: Frust `frust-database` (in-process `rusqlite`), Flutter
  `package:sqlite3` (engine-parity, in-process FFI), and Flutter
  `sqflite` (ecosystem-typical, platform channel) — PROTOCOL §9.7. A
  fourth `frust-database` (`turso`) column is added later, once that
  backend lands.
- A `d1` table: `insert_batch` / `insert_single` p50/p95/p99 op latency
  (µs) + ops/s (PROTOCOL §9.3, §9.6).
- A `d2` table: `select_point` / `range_scan` p50/p95/p99 op latency
  (µs) + ops/s (PROTOCOL §9.4, §9.6).
- Each side's exact linked SQLite version, recorded per run (not assumed
  from a package's declared minimum — PROTOCOL §9.7).
- A methodology-deviations subsection, same discipline as every device
  block above.
