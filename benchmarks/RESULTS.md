# Benchmark Results

Results record for the paired Frust-vs-Flutter benchmark suite, per
`PROTOCOL.md`. **A device or scenario with no completed runs is omitted, not
filled with placeholder numbers.** This file holds the Frust-vs-Flutter matrix
only; engine-plan gate evidence (render-tier A/Bs, spikes, desktop stress,
wgpu-pin smokes) that used to sit here was retired to git history on
2026-09-05 (`git show f64be636:benchmarks/RESULTS.md`).

Every table below is reproducible from `benchmarks/raw/<device>/<app>/<sN>/`
(sanitized per-run series committed alongside this file) with
`python3 benchmarks/harness/summarize.py --frust <…>/frust_profile --flutter <…>/flutter`,
which runs `harness/stats.py` — the one shared statistics script — over both
apps' raw lines. Percentiles are nearest-rank over non-skipped frames; the first
2 of 12 runs are warm-up and excluded; miss counts are per frame attempted. Each
device's "vs 2026-09-05" table is `harness/compare.py` over two of those JSON
dumps, both regenerated from the committed raw trees; Δ positive = slower.

> **Post-optimization pass, 2026-09-06.** Every Frust series below for the
> OnePlus 9, iPhone SE and Pixel 5 is a `--profile` build of `21076221` on the
> frust-owned `frust-engine` strip pipeline with the render-thread split, and
> replaces that device's 2026-09-05 series (PROTOCOL §2.6); each device section
> carries a compact delta table against it. **All three devices ran the full
> 16 blocks**, in one unattended `matrix.sh` session each: OnePlus 9 (USB,
> 120 Hz pinned), iPhone SE (network `devicectl`), Pixel 5 (adb-over-Wi-Fi,
> unpinned). Frust's `vello` renderer was deleted on 2026-09-02 and the
> 2026-07-21 vello-era matrix retired with it; the Xiaomi 12 section below is
> the condensed remnant of that pass and the one device still awaiting a re-run.
> Toolchain for this pass: Rust 1.98.1 (pinned), Flutter 3.47.2 stable /
> Dart 3.13.2, Xcode 26.2 (17C52), `wgpu` 30.0.1 (pinned).

## Success criteria — post-optimization pass, 2026-09-06

Every criterion the optimization work set itself, scored with a number from this pass. Two are missed and are recorded as missed; one was written against a quantity that no longer exists and is restated rather than scored.

| Criterion | Measured this pass | Verdict |
|---|---|---|
| iPhone SE framework first frame ≤ 85 ms | **100 ms** median (64–176 over 12 launches) | **MISS** |
| OnePlus 9 S1 `gpu_main` ≤ 5.0 ms | **6.37 ms** p50 over 35,989 `gpu_q=1` frames | **MISS** |
| S3 "two vsyncs ≤ 0.5 %" | criterion is void — see below | **RESTATED** |
| S5 continuous image column, both Android devices | OnePlus `skipped=0` over 34,878 `img` lines; Pixel emits **no `img` line at all** | **MET** |
| S3 `create 1k` > 0 frames on every device | OnePlus **341**, iPhone SE **180**, Pixel 5 **260** | **MET** |

**iPhone SE first frame — missed on the median, met on half the launches.** 100 ms median against the 85 ms bar, better than 2026-09-05's 110 ms on every statistic (min 78→64, max 250→176). The distribution is bimodal — six launches at 64–82 ms, six at 119–176 — and the split is entirely the font preinit: `first_frame_presented` = `font_preinit_joined` + 13 ms ±1 in all twelve launches, the join being 51–65 ms in the fast half and 100–159 in the slow. GPU bring-up is no longer the constraint (`adapter_ready` 102→22 ms median, max 239→34), so closing this is a font-preinit question, not a renderer one.

**OnePlus 9 S1 `gpu_main` — missed, and flat against the baseline.** 6.37 ms against the 5.0 ms bar, where 2026-09-05 measured 6.34: no regression and no improvement. S1 is fragment-shader bound here — 6.37 of the frame's 6.45 ms GPU total is `gpu_main`, prepass 0.07, composite/blit 0.00 — so the identified lever is in the shader and is not in this build. The Pixel 5 reads `gpu_main` 13.60 ms against an 11.11 ms panel period, i.e. GPU-bound there too.

**"S3 two vsyncs ≤ 0.5 %" is void as written.** It counted S3 frames whose `total_us` exceeded two vsync periods. Under the render-thread split `total_us` sums the UI half's `rebuild`/`layout`/`paint` with the render half's `encode`/`acquire`/`submit`, folded by `FramePasses::from_split` from two threads that run **concurrently** for one frame (PROTOCOL §7) — a per-frame cost sum, not the interval between presents. A frame can cost more than a period and still present on the next vsync, so "over two budgets" counts nothing. This pass proves it arithmetically: OnePlus 9 S1 has a cost-sum p50 of 8.55 ms with 63.05 % of frames above the 8.33 ms column, while the same capture presents 36,029 frames in 300 s — 120.1 fps, a cadence a 63 % two-vsync rate cannot coexist with. A per-frame present-to-present *distribution* is not recoverable either: the sanitized logs carry no timestamps and `platform-view … period_us` is a smoothed display-period estimate, not an interval sample. **Restated as the two real quantities:** S3 cost-sum p95 and achieved present rate.

| Device | S3 cost-sum p95 old→new | S3 present rate old→new |
|---|---|---|
| OnePlus 9 (120 Hz pinned) | 12.16 → 12.26 ms | 114.5 → 114.6 fps |
| iPhone SE (60 Hz) | 18.78 → 18.48 ms | 60.9 → 61.4 fps |
| Pixel 5 (unpinned 60/90 Hz) | 9.60 → 10.84 ms | 89.0 → 88.7 fps |

Every device holds its present rate to within 0.5 fps of the previous pass, and the cost-sum p95 is flat on the OnePlus 9, better on the iPhone SE and worse on the Pixel 5 — the last inside that device's own between-session variance (see its delta note).

---

## Device: Pixel 5 (redfin, Snapdragon 765G / Adreno 620) — mid-tier Android

**Status:** run 2026-09-06 17:55–19:59 UTC, **12 runs × 30 s per scenario per app (S7: 60 s runs), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** **Frust = PROFILE build of `21076221`** (engine renderer with the render-thread split; `frust build … --profile --define FRUST_TRACE_RAW=1`, raw format v4), **Flutter = profile build** (`flutter build … --profile`, Flutter 3.47.2). Raw series under `raw/pixel5/frust_profile/<sN>/` and `raw/pixel5/flutter/<sN>/` (run-NN.log + run-NN.pss_before/after.txt, stats.txt; S7 adds cpuinfo.txt/coldstart.txt). One unattended `matrix.sh` session, all 16 blocks captured on attempt 1, no thermal stall; the frust S8 block was re-run once by a stale driver check that does not affect its data (deviation 2). The device was reached over **adb-over-Wi-Fi** (deviation 1). **This pass supersedes the 2026-09-05 pass on this device.**

- Chipset: Snapdragon 765G (board `lito`) / Adreno 620. Model: Pixel 5 `redfin` (serial redacted).
- Display: 1080×2340 @ ~435 dpi; modes id 0 = 60 Hz, id 1 = 90 Hz. **Refresh NOT pinned** (`peak_refresh_rate`/`min_refresh_rate` unset throughout, matching this device's earlier passes); the apps opt in themselves. **The panel was already in the 90 Hz mode at session start** (`mActiveSfDisplayMode` id 1), where the 2026-09-05 session recorded 60 Hz at idle. stats.py budgets are 16.67 / 8.33 ms only — the panel's 11.11 ms (90 Hz) budget is not a column (deviation 3).
- High-refresh opt-in engaged on both apps (§6): Frust `Surface.setFrameRate`, Flutter `flutter_displaymode`. Achieved (active frames ÷ 300 s): Frust ~88.4–88.7 fps in S2/S3/S4/S6, ~79.4 in S5, ~65.9 in S1; Flutter ~82.0 (S1), ~84.0 (S5), ~87.5–87.8 (S4/S6); S2/S3 Flutter paint is event-driven.
- Brightness fixed 128/255, auto-brightness off (`device_state.sh`) for every block. Screen timeout 30 min, `svc power stayon true` during blocks. Battery saver off.
- Radios: **left exactly as found by `--wireless`** (airplane setting untouched, Wi-Fi up, a tunnel carrying the adb link) — not the §3 radios-off state (deviation 1).
- Charger: **AC-connected for the whole session** (`AC powered: true`), real level 100 % → 92 %; `dumpsys battery unplug` spoofed on-battery state before every block. **The 2026-09-05 session ran on battery (99 % → 78 %)** — see the delta note below, this is a real environmental difference between the two passes.
- Thermal: gate ceiling 38 °C / 120 s cooldown before every block; session start 28.6 °C, peak 37.2 °C (after the frust S6 block), end 31.0 °C. **No cooldown stall in any block** (no `warning: still` in any run.log). 30 s blocks 384–386 s wall, 60 s S7 blocks 744/743 s.
- Toolchain: Rust 1.98.1 (pinned), `wgpu` 30.0.1 (pinned); Flutter 3.47.2 stable / Dart 3.13.2. Frust APK arm64-only, 12,008,167 B (md5 1715cb35…); Flutter APK fat profile build, 70,462,089 B (md5 43132929…) — packaged differently, so their APK sizes are not a size comparison. Both installed fresh, md5 re-verified by the driver.
- Visual gate (driver screencap per block, all 16 reviewed): **PASS on every block, both apps**, and no keyboard or other system window intruded on any capture (bottom-band luma 20–89 across all 16, against the 237 that a Gboard window reads — the OnePlus 9's deviation 1 did not occur here). **S5 geometry PASS — Frust's column is edge-to-edge with every visible cell fully decoded, no placeholders and no gaps.**
- Sanitization: driver staging `RAW_OK` (192 run logs, 384 PSS snapshots, 16 stats.txt, S7 cpuinfo/coldstart), whitelist self-check passed.

### S1

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 15.23 | 18.03 | 19.00 | 146.39 | 2,016 (10.20%) | 19,765 (100.00%) | 19,765 (~65.9 fps avg) |
| Flutter (profile) | 12.55 | 20.21 | 23.54 | 68.05 | 1,434 (5.83%) | 24,504 (99.59%) | 24,604 (~82.0 fps avg) |

Frust pass p50 (ms): rebuild 0.11, layout 0.00, paint 0.19, encode 0.08, acquire 0.03, submit 14.52; layout_us>0 on 6,903/19,765 frames; GPU (gpu_q=1 on 19,725) total p50/p95 13.91/14.15 ms


**Split, and this is the device's one GPU-bound scenario.** Flutter takes the median (12.55 vs 15.23 ms) and the worst frame (68.05 vs 146.39); Frust takes the tail (p95 18.03 vs 20.21, p99 19.00 vs 23.54). Frust misses the 60 Hz budget on 10.20 % of frames against Flutter's 5.83 %, at ~65.9 vs ~82.0 fps. The breakdown says why plainly: 14.52 of Frust's 15.23 ms is `submit`, and GPU pass time is 13.91 ms median — above the panel's 11.11 ms period at 90 Hz, so S1 cannot hold the refresh rate on Adreno 620 no matter what the CPU side does. `gpu_main` is 13.60 ms of that 13.91.
### S2

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 10.18 | 11.32 | 14.14 | 158.28 | 135 (0.51%) | 17,621 (66.45%) | 26,517 (~88.4 fps avg) |
| Flutter (profile) | 31.28 | 40.59 | 66.64 | 83.79 | 3,152 (99.49%) | 3,168 (100.00%) | 3,168 (~10.6 fps avg) |

Frust pass p50 (ms): rebuild 0.09, layout 3.73, paint 0.35, encode 0.06, acquire 0.07, submit 5.95; layout_us>0 on 26,516/26,517 frames; GPU (gpu_q=1 on 26,477) total p50/p95 8.15/8.44 ms


**Frust wins S2 decisively** — p50 10.18 vs 31.28 ms, p95 11.32 vs 40.59, and 0.51 % vs 99.49 % of frames over the 60 Hz budget. Frust repaints continuously at ~88.4 fps (26,517 frames) while Flutter manages 3,168 painted frames; the per-frame figures compare, the frame counts do not. Frust re-lays out every frame at 3.73 ms median, the most expensive layout in the matrix.
### S4

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 5.93 | 11.44 | 12.45 | 148.06 | 44 (0.17%) | 3,022 (11.37%) | 26,586 (~88.6 fps avg) |
| Flutter (profile) | 5.17 | 6.74 | 8.11 | 29.48 | 6 (0.02%) | 177 (0.67%) | 26,334 (~87.8 fps avg) |

Frust pass p50 (ms): rebuild 0.24, layout 0.00, paint 0.12, encode 0.03, acquire 1.70, submit 3.79; layout_us>0 on 23,840/26,586 frames; GPU (gpu_q=1 on 26,546) total p50/p95 3.94/4.32 ms


**Flutter wins S4** — p50 5.17 vs 5.93 ms, p95 6.74 vs 11.44, worst 29.48 vs 148.06 — with both apps near the panel rate and both essentially never missing the 60 Hz budget (0.02 % vs 0.17 %).
### S5

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 13.04 | 13.84 | 14.62 | 164.75 | 108 (0.45%) | 23,450 (98.49%) | 23,809 (~79.4 fps avg) |
| Flutter (profile) | 8.39 | 12.57 | 15.37 | 30.05 | 105 (0.42%) | 12,845 (50.95%) | 25,211 (~84.0 fps avg) |

Frust pass p50 (ms): rebuild 0.62, layout 0.03, paint 0.03, encode 0.02, acquire 0.06, submit 12.18; layout_us>0 on 23,808/23,809 frames; GPU (gpu_q=1 on 23,769) total p50/p95 12.15/12.44 ms


**Split, on a valid image column.** Flutter wins the median (8.39 vs 13.04 ms); Frust wins the tail (p95 13.84 vs 12.57 is Flutter's, but p99 14.62 vs 15.37 and the 60 Hz miss rate 0.45 % vs 0.42 % are a wash) and the two are within noise on budget misses. The content gate is the strongest form available: **no `frust-perf img` line was emitted at all across the kept runs**, which per PROTOCOL §7 means the atlas never skipped or evicted an image — the line only appears when one of those counters is non-zero. 12.18 of Frust's 13.04 ms is `submit` against 12.15 ms of GPU pass time, so this scenario is GPU-bound here too.
### S6

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 7.88 | 10.15 | 13.43 | 166.62 | 99 (0.37%) | 8,271 (31.20%) | 26,511 (~88.4 fps avg) |
| Flutter (profile) | 10.25 | 17.96 | 19.65 | 37.32 | 2,465 (9.39%) | 24,835 (94.63%) | 26,243 (~87.5 fps avg) |

Frust pass p50 (ms): rebuild 0.11, layout 0.20, paint 0.32, encode 0.11, acquire 1.25, submit 5.78; layout_us>0 on 26,511/26,511 frames; GPU (gpu_q=1 on 26,471) total p50/p95 7.84/9.44 ms


**Frust wins S6** — p50 7.88 vs 10.25 ms, p95 10.15 vs 17.96, p99 13.43 vs 19.65, and 0.37 % vs 9.39 % of frames over the 60 Hz budget, at the same ~88 fps.
### S3

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | 12.70 / 19.36 / 20.48 / 21.02 | 260 | 5.67 / 7.34 / 7.34 / 7.34 | 4 |
| create 10k | 9.85 / 14.21 / 17.86 / 23.73 | 270 | **not captured** | 0 |
| update every 10th of 10k | 8.57 / 10.59 / 13.72 / 17.77 | 269 | **not captured** | 0 |
| swap | 8.47 / 10.42 / 15.90 / 16.37 | 270 | **not captured** | 0 |
| clear | 7.67 / 9.24 / 12.05 / 14.17 | 260 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 6.61 | 10.84 | 13.21 | 144.59 | 140 (0.53%) | 3,965 (14.90%) | 26,604 (~88.7 fps avg) |
| Flutter (profile) | 19.34 | 54.08 | 59.12 | 67.91 | 470 (52.40%) | 851 (94.87%) | 897 (~3.0 fps avg) |

Cycle health — Frust (profile) `s3-create1k` reopens per kept run: 27–27; Flutter (profile) `s3-create1k` reopens per kept run: 18–18.


**Frust wins S3, and all five reconcile ops are captured.** Each op lands 260–270 frames under the frame-indexed markers (create 1k 12.70 ms p50, create 10k 9.85, update 8.57, swap 8.47, clear 7.67); Flutter lands 4 frames on `create 1k` and none elsewhere, so the per-op columns are not a race. On the overall series Frust is 6.61 ms p50 / 10.84 p95 over 26,604 painted frames against Flutter's 19.34 / 54.08 over 897.
### S7

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | ~159 ms (164/159/144; first of 4 discarded) | ~638 ms (611/651/638; first of 4 discarded) |
| Framework-reported first-frame span | ~172 ms median (`first_frame_presented`, 161–186 across 12 launches; `adapter_ready` median 43 ms) | ~162 ms median (`first_frame_ms`, 158–216 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | 0 samples | 0 samples |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~96.5 MB (96.0–96.8, 10 kept snapshots) | ~120.9 MB (117.9–129.0, 10 kept snapshots) |


**Frust starts ~4× faster and idles ~20 % lighter.** External cold start 159 vs 638 ms (`am start -W` TotalTime), idle TOTAL PSS 96.5 vs 120.9 MB. The framework first-frame spans are close on this device (172 vs 162 ms median) and, as always, are measured from each framework's own entry point. Neither app appeared in `dumpsys cpuinfo` during the idle window, i.e. idle CPU ~0 % for both.
### S8

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (profile) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 136 | 405 | n/a |
| write i64 | 148 | 419 | n/a |
| write f64 | 162 | 462 | n/a |
| write String | 150 | 407 | n/a |
| write Vec\<String\> | 153 | 425 | n/a |
| read (unique key, forces channel) bool | 41 | 15300† | ~1 |
| read (unique key, forces channel) i64 | 41 | 15300† | ~0 |
| read (unique key, forces channel) f64 | 41 | 15300† | ~1 |
| read (unique key, forces channel) String | 41 | 15300† | ~1 |
| read (unique key, forces channel) Vec\<String\> | 42 | 15300† | ~2 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust wins S8 decisively** — writes 136–162 vs 405–462 µs/call (~2.8–3.0×), and a per-key Frust read at 41 µs against Flutter's whole-store `reload()` at 15,300 µs. This is the slowest S8 of the three devices on both sides, which is the storage tier rather than either framework. Zero `s8-errors` on either app.

### vs 2026-09-05 (Frust, same device, same scenarios)

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new (ms) | Δp95 | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|
| S1 | 14.86→15.23 | +2.5% | 16.18→18.03 | +11.4% | 1.72%→10.20% | 67.0→65.9 |
| S2 | 8.91→10.18 | +14.3% | 10.61→11.32 | +6.7% | 0.13%→0.51% | 89.0→88.4 |
| S4 | 5.70→5.93 | +4.0% | 7.32→11.44 | +56.4% | 0.09%→0.17% | 89.0→88.6 |
| S5 | 13.19→13.04 | -1.1% | 13.99→13.84 | -1.1% | 0.14%→0.45% | 79.0→79.4 |
| S6 | 6.82→7.88 | +15.7% | 8.71→10.15 | +16.5% | 0.09%→0.37% | 88.8→88.4 |
| S3 (overall) | 5.97→6.61 | +10.7% | 9.60→10.84 | +12.9% | 0.16%→0.53% | 0.0→0.0 |

**Read this table with the control beside it, not on its own.** Frust moved +2.5…+15.7 % on p50 (S5 −1.1 %), but the **Flutter control on the same runs moved −39.5 % to +38.1 %** — S2 p50 22.65→31.28 (+38.1 %), S4 p95 18.55→6.74 (−63.7 %), S6 p50 16.94→10.25 (−39.5 %), S5 p50 6.27→8.39 (+33.9 %). An unchanged control app swinging by up to 64 % between the two sessions means this device's session-to-session variance is several times the Frust delta, so **no per-scenario conclusion should be drawn from the Frust column here**. Two environmental differences are documented and plausibly responsible: the 2026-09-05 session ran **on battery** (99 → 78 %) while this one ran **on AC** (100 → 92 %), and the panel idled at **90 Hz** this session against 60 Hz last time — on an unpinned device both feed directly into the DVFS and refresh policy the frame loop runs under. Off the frame axis, where the metrics are not per-frame, the pass is better: external cold start 137→159 ms is worse but idle TOTAL PSS falls 104.7→96.5 MB (Frust) and 142.4→120.9 MB (Flutter). A clean re-measurement of this device with the charger state matched to the baseline is the way to settle the frame rows.

### Methodology deviations (this device)

1. **adb-over-Wi-Fi, radios left as found.** The device was reached over `--wireless`, so airplane mode, Wi-Fi and Bluetooth were not touched (toggling them would cut the link) and the §3 radios-off state was not applied. Same deviation as this device's 2026-09-05 pass.
2. **frust S8 graded `DEGENERATE` twice by a stale driver check; the data is complete.** Identical to the other two devices: `matrix.sh`'s `block_sanity` counts `-perf plugin op=` while Frust's S8 emitters now lead with the canonical inline `scenario=` key, so the check saw 2 lines where the run captured 2,002 per-op lines. `stats.py` parses the new shape and the S8 table above is computed from it; `pick_src` fell back to attempt 1. The one-line driver-pattern fix is owed.
3. **The 90 Hz budget is not a column.** stats.py reports the 16.67 and 8.33 ms budgets only, so on this unpinned 60/90 Hz panel neither column is the frame period the apps were actually running to (11.11 ms at 90 Hz). Read the achieved-fps figures for cadence.
4. **Charger state differs from the 2026-09-05 baseline** — AC throughout here, battery-only there. Recorded above because it bounds the delta table, not because it invalidates this pass's own Frust-vs-Flutter comparison, where both apps shared the identical session.
5. **Flutter S2 rows are not comparable to any Flutter S2 row before 2026-09-05** — the Flutter bench app was changed to single-line ellipsis for row-text parity with Frust.
6. As on prior passes: Flutter release-mode in-app cross-check not captured; S4 parse wall time not recoverable (timestamp-free capture); S8 burst-during-animation variant not run; S3 Flutter per-op capture lands 4 frames on `create 1k` and none elsewhere (the known `addTimingsCallback` delivery gap).

---

## Device: OnePlus 9 (LE2115 "lemonade", Snapdragon 888 / Adreno 660) — mid-tier Android

**Status:** run 2026-09-06 13:26–15:32 UTC, **12 runs × 30 s per scenario per app (S7: 60 s runs), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** **Frust = PROFILE build of `21076221`** (engine renderer with the render-thread split; `frust build … --profile --define FRUST_TRACE_RAW=1`, raw format v4), **Flutter = profile build** (`flutter build … --profile`, Flutter 3.47.2). Raw series under `raw/oneplus9/frust_profile/<sN>/` and `raw/oneplus9/flutter/<sN>/` (run-NN.log + run-NN.pss_before/after.txt, stats.txt; S7 adds cpuinfo.txt/coldstart.txt). One unattended `matrix.sh` session, all 16 blocks captured; 15 graded `ok` on attempt 1 and the frust S8 block was re-run once by a stale driver check that does not affect its data (deviation 2). **This pass supersedes the 2026-09-05 pass on this device**, whose S5 row is invalid (deviation 1 of that pass, restated in deviation 10 below).

- Chipset: Snapdragon 888 (board `lahaina`) / Adreno 660, GLES driver V@0530.53. Model: OnePlus 9 LE2115 (serial redacted).
- OS: LineageOS — Android 15, `lineage_lemonade-userdebug 15 BP1A.250405.007` (nightly 20250405), SDK 35; fingerprint still reports `OnePlus9:14/UKQ1.230924.001`.
- Display: 1080×2400 @ ~392 dpi; modes id 0 = 60 Hz (active at idle), id 1 = 120 Hz. **Both `min_refresh_rate`/`peak_refresh_rate` pinned to 120 for every block** (`--pin-refresh 120`, read-back `peak=120.0 min=120.0`, restored to `Infinity`/unset afterwards); Frust's in-app FPS readout showed 121.0 in S1. Both budgets (16.67 / 8.33 ms) reported — but see the note under "vs 2026-09-05" on what the 8.33 ms column now means under the render-thread split.
- High-refresh opt-in engaged on both apps (§6): Frust `Surface.setFrameRate`, Flutter `flutter_displaymode`. Achieved (active frames ÷ 300 s): Frust ~120.1 fps in every frame scenario; Flutter ~97.8 (S1), ~108.6 (S5), ~119.2 (S4/S6); S2/S3 Flutter paint is event-driven.
- Brightness fixed 128/255, auto-brightness off (`device_state.sh`); airplane on (read-back 1), Wi-Fi off, Bluetooth off, battery saver off, screen timeout 30 min, `svc power stayon true` during blocks; all restored and read back at 15:32 UTC (airplane 0, Wi-Fi on, BT off, `peak_refresh_rate` Infinity, `min_refresh_rate` unset).
- Charger: USB-connected throughout (adb); `dumpsys battery unplug` spoofed on-battery state before every block. Real level 100 % for all 16 blocks, so the `--min-level 30` guard never waited.
- Thermal: gate ceiling 38 °C / 120 s cooldown before every block. Session start 31.2 °C, peak 38.2 °C (after the flutter S5 block), end 32.6 °C. **One cooldown timeout**, in the frust S6 block (deviation 3); no other block stalled. 30 s blocks 384–385 s wall (S6 frust 506 s), 60 s S7 blocks 744 s each.
- Toolchain: Rust 1.98.1 (pinned), `wgpu` 30.0.1 (pinned); Flutter 3.47.2 stable / Dart 3.13.2. Frust APK arm64-only, 12,008,167 B (md5 1715cb35…); Flutter APK fat profile build, 70,462,089 B (md5 43132929…) — the two are packaged differently and their APK sizes are not a size comparison (see the App size section). Stale installs removed and both installed fresh by the driver, md5 re-verified.
- Visual gate (driver screencap per block, all 16 reviewed): **content PASS on every block, both apps.** S1 the same deterministic bubble field on both; S2 mid-scroll rows on both; S3 populated table on both; S4 parse screen on both; **S5 geometry PASS — Frust's column is edge-to-edge and every visible cell fully decoded, Flutter's shows three decoded cells and one grey placeholder**; S6 multilingual block on both; S7 idle pages; S8 quiescent screens. A soft-keyboard window overlays the lower third of all eight **frust** captures and none of the flutter ones — deviation 1, which is about composition, not content.
- Sanitization: driver staging `RAW_OK` (192 run logs, 384 PSS snapshots, 16 stats.txt, S7 cpuinfo/coldstart), whitelist self-check passed.

### S1

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 8.55 | 9.84 | 10.61 | 54.73 | 39 (0.11%) | 22,717 (63.05%) | 36,029 (~120.1 fps avg) |
| Flutter (profile) | 9.50 | 16.96 | 19.47 | 87.02 | 1,501 (5.12%) | 26,519 (90.39%) | 29,337 (~97.8 fps avg) |

Frust pass p50 (ms): rebuild 0.19, layout 0.00, paint 0.42, encode 0.06, acquire 1.61, submit 6.25; layout_us>0 on 243/36,029 frames; GPU (gpu_q=1 on 35,989) total p50/p95 6.45/6.72 ms


**Frust wins S1** — p50 8.55 vs 9.50 ms, p95 9.84 vs 16.96, p99 10.61 vs 19.47, worst 54.73 vs 87.02, and 0.11 % vs 5.12 % of frames over the 60 Hz budget, at ~120.1 vs ~97.8 fps. The pass breakdown puts 6.25 ms of Frust's 8.55 ms in submit and 6.45 ms of GPU pass time under it — S1 is GPU-bound on this device (see the criteria section: `gpu_main` p50 6.37 ms).
### S2

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 9.06 | 11.31 | 12.86 | 53.71 | 44 (0.12%) | 28,884 (80.19%) | 36,021 (~120.1 fps avg) |
| Flutter (profile) | 15.46 | 24.75 | 28.25 | 49.27 | 2,301 (39.15%) | 5,103 (86.82%) | 5,878 (~19.6 fps avg) |

Frust pass p50 (ms): rebuild 0.10, layout 1.80, paint 0.28, encode 0.04, acquire 0.63, submit 5.95; layout_us>0 on 36,019/36,021 frames; GPU (gpu_q=1 on 35,981) total p50/p95 4.85/5.04 ms


**Frust wins S2 decisively** — p50 9.06 vs 15.46 ms, p95 11.31 vs 24.75, p99 12.86 vs 28.25, and 0.12 % vs 39.15 % of frames over the 60 Hz budget. Frust holds ~120.1 fps continuously while Flutter's paint is event-driven (~19.6 fps of painted frames), so the fps columns are not comparable; the per-frame figures are. Frust re-lays out every frame here (`layout_us>0` on 36,019/36,021) at 1.80 ms median.
### S4

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.77 | 6.17 | 8.35 | 71.41 | 28 (0.08%) | 367 (1.02%) | 36,046 (~120.2 fps avg) |
| Flutter (profile) | 4.22 | 4.63 | 5.77 | 20.03 | 1 (0.00%) | 4 (0.01%) | 35,759 (~119.2 fps avg) |

Frust pass p50 (ms): rebuild 0.11, layout 0.00, paint 0.06, encode 0.02, acquire 0.10, submit 4.27; layout_us>0 on 17/36,046 frames; GPU (gpu_q=1 on 36,006) total p50/p95 1.12/1.26 ms


**Flutter wins S4** — p50 4.22 vs 4.77 ms, p95 4.63 vs 6.17, p99 5.77 vs 8.35, worst 20.03 vs 71.41, both at ~120 fps and both essentially never missing the 60 Hz budget (0.00 % vs 0.08 %). The concurrent animation costs Frust more per frame than Flutter under the same JSON parse.
### S5

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 7.93 | 8.89 | 10.51 | 63.45 | 32 (0.09%) | 6,415 (17.81%) | 36,018 (~120.1 fps avg) |
| Flutter (profile) | 6.43 | 14.26 | 16.34 | 69.64 | 250 (0.77%) | 10,675 (32.78%) | 32,570 (~108.6 fps avg) |

Frust pass p50 (ms): rebuild 1.06, layout 0.06, paint 0.08, encode 0.01, acquire 1.32, submit 5.40; layout_us>0 on 36,018/36,018 frames; GPU (gpu_q=1 on 35,978) total p50/p95 6.45/6.61 ms


**Split, and this row is the one that changed meaning.** Flutter wins the median (6.43 vs 7.93 ms); Frust wins the tail (p95 8.89 vs 14.26, p99 10.51 vs 16.34), the 60 Hz miss rate (0.09 % vs 0.77 %) and the cadence (~120.1 vs ~108.6 fps). Unlike the 2026-09-05 row, **this capture composites the whole image column**: 34,878 `frust-perf img` lines across the kept runs report `skipped=0` on every one, with 55,317 evictions and up to 63 resident images against a 1024×1024×4 atlas — PROTOCOL §7's content gate ("no line at all, or lines with `skipped=0`") is met and eviction-only degradation is the documented healthy mode. Frust's rebuild rises to 1.06 ms median, which is that re-upload traffic.
### S6

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 8.44 | 9.16 | 11.01 | 58.26 | 51 (0.14%) | 21,577 (59.88%) | 36,034 (~120.1 fps avg) |
| Flutter (profile) | 8.30 | 9.81 | 10.31 | 95.67 | 2 (0.01%) | 17,443 (48.79%) | 35,750 (~119.2 fps avg) |

Frust pass p50 (ms): rebuild 0.15, layout 0.31, paint 0.46, encode 0.04, acquire 1.21, submit 6.17; layout_us>0 on 36,034/36,034 frames; GPU (gpu_q=1 on 35,994) total p50/p95 1.73/1.78 ms


**Near-tie on S6.** Flutter takes the median by a hair (8.30 vs 8.44 ms) and the 60 Hz miss rate (0.01 % vs 0.14 %); Frust takes p95 (9.16 vs 9.81) and the worst frame (58.26 vs 95.67). Frust's GPU pass time is 1.73 ms median — the cost here is shaping and upload on the CPU side, not the strip pipeline.
### S3

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | 14.98 / 19.07 / 20.60 / 54.69 | 341 | 10.48 / 10.65 / 10.65 / 10.65 | 4 |
| create 10k | 20.62 / 21.75 / 22.04 / 22.21 | 346 | **not captured** | 0 |
| update every 10th of 10k | 15.57 / 19.20 / 19.75 / 20.50 | 350 | **not captured** | 0 |
| swap | 14.54 / 19.14 / 20.47 / 22.01 | 348 | **not captured** | 0 |
| clear | 15.18 / 17.17 / 18.58 / 18.93 | 340 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 5.66 | 12.26 | 18.98 | 63.23 | 658 (1.91%) | 2,592 (7.54%) | 34,371 (~114.6 fps avg) |
| Flutter (profile) | 19.22 | 33.85 | 35.66 | 90.35 | 608 (65.87%) | 905 (98.05%) | 923 (~3.1 fps avg) |

Cycle health — Frust (profile) `s3-create1k` reopens per kept run: 35–35; Flutter (profile) `s3-create1k` reopens per kept run: 19–19.


**Frust wins S3 on every comparable axis, and the per-op rows are readable again.** All five Frust reconcile ops now land frames (340–350 each) because the bench app stamps `bench-scenario-start n=<frame>` markers that `stats.py` slices by frame index; on 2026-09-05 `create 1k` collected 0 frames and each other op's bucket held the previous op's frame. Flutter still lands only 4 frames, on `create 1k` alone (its `addTimingsCallback` delivery gap), so the per-op columns are not a like-for-like race. On the overall series Frust is at 5.66 ms p50 / 12.26 p95 over 34,371 painted frames against Flutter's 19.22 / 33.85 over 923 — continuous vs event-driven paint, so read the per-op table for the reconcile cost and the overall table only for cadence.
### S7

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | ~128 ms (141/128/126; first of 4 discarded) | ~422 ms (422/430/409; first of 4 discarded) |
| Framework-reported first-frame span | ~95 ms median (`first_frame_presented`, 89–104 across 12 launches; `adapter_ready` median 36 ms) | ~79 ms median (`first_frame_ms`, 66–86 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | 0 samples | 0 samples |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~116.8 MB (116.5–117.2, 10 kept snapshots) | ~157.4 MB (157.1–157.7, 10 kept snapshots) |


**Frust starts ~3.3× faster and idles ~26 % lighter.** External cold start 128 vs 422 ms (`am start -W` TotalTime), idle TOTAL PSS 116.8 vs 157.4 MB. Flutter reports the shorter framework first-frame span (79 vs 95 ms median), measured from each framework's own entry point, so the two spans bound different work; the external `am start` figure is the one measured identically on both sides. Neither app appeared in `dumpsys cpuinfo` during the idle window (0 samples both), i.e. idle CPU ~0 % for both.
### S8

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (profile) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 99 | 291 | n/a |
| write i64 | 93 | 289 | n/a |
| write f64 | 93 | 309 | n/a |
| write String | 93 | 276 | n/a |
| write Vec\<String\> | 92 | 281 | n/a |
| read (unique key, forces channel) bool | 19 | 4378† | ~0 |
| read (unique key, forces channel) i64 | 19 | 4378† | ~0 |
| read (unique key, forces channel) f64 | 19 | 4378† | ~0 |
| read (unique key, forces channel) String | 19 | 4378† | ~0 |
| read (unique key, forces channel) Vec\<String\> | 19 | 4378† | ~0 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust wins S8 decisively** — writes 92–99 vs 276–309 µs/call (~3.0–3.3×); a per-key Frust read costs 19 µs while Flutter's only channel-crossing read is the whole-store `reload()` at 4,378 µs (its ~0 µs cached read is a Dart-map lookup, not a boundary crossing). Zero `s8-errors` on either app.

### vs 2026-09-05 (Frust, same device, same scenarios)

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new (ms) | Δp95 | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|
| S1 | 8.20→8.55 | +4.2% | 9.45→9.84 | +4.2% | 0.10%→0.11% | 120.1→120.1 |
| S2 | 8.83→9.06 | +2.6% | 11.45→11.31 | -1.2% | 0.10%→0.12% | 120.0→120.1 |
| S4 | 4.54→4.77 | +5.0% | 6.22→6.17 | -0.7% | 0.07%→0.08% | 120.2→120.2 |
| S5 | 7.78→7.93 | +2.0% | 8.85→8.89 | +0.5% | 0.10%→0.09% | 120.2→120.1 |
| S6 | 8.19→8.44 | +3.0% | 8.90→9.16 | +2.9% | 0.12%→0.14% | 120.2→120.1 |
| S3 (overall) | 5.57→5.66 | +1.7% | 12.16→12.26 | +0.8% | 2.14%→1.91% | 0.0→0.0 |

Frust's per-frame cost sum is 2–5 % higher on every scenario while the presented cadence is unchanged at ~120.1 fps, and the Flutter control moved −1.7…+2.3 % on the same runs, so the control is stable and the Frust move is real but small. Two caveats bound how far it can be read: only the frust blocks carried an extra composited system layer (deviation 1), and Frust's `acquire_us` — the swapchain wait, which that layer would lengthen — accounts for most of the difference (S1 1.28→1.61 ms, S5 1.12→1.32, S6 0.96→1.21). **S5 is not a regression at all**: it is +2.0 % while compositing an image column the 2026-09-05 capture dropped roughly half of. Off the frame axis the pass is faster: external cold start 137→128 ms, framework first frame 105→95 ms, `adapter_ready` 50→36 ms. Idle TOTAL PSS rose 104.7→116.8 MB (Frust) and 142.4→157.4 MB (Flutter), i.e. on both sides, so it is not a Frust-side change.

**The 8.33 ms column counts cost, not vsync misses** — under the render-thread split `total_us` is a two-thread cost sum, and this device's S1 is the worked example in the criteria section above (8.55 ms p50, 63.05 % "over budget", 120.1 fps presented).

### Methodology deviations (this device)

1. **Soft-keyboard window over the frust blocks only (composition asymmetry).** A Gboard window (`com.google.android.inputmethod.latin`) covers the lower third of all eight frust block screencaps and none of the eight flutter ones (bottom-band mean luma 237 vs 20–81 across the 16 captures). It did not change what either app drew: `dumpsys window` reports the bench window at the full `[0,0][1080,2400]` with `insetsChanged=false`, and the frust captures show content clipped mid-bubble and mid-cell at the keyboard edge rather than re-laid out, so both apps rendered their whole surface and every per-frame span measures the same scene. What it does add is one more layer for SurfaceFlinger to composite over the frust blocks, which is the most likely source of the `acquire_us` rise above. It therefore makes this device's Frust-vs-2026-09-05 deltas a soft comparison and its **Frust-vs-Flutter verdicts conservative for Frust**, since only Frust carried the extra layer. The condition did not reproduce after the session (`mInputShown=false`, both apps' post-run screencaps clean), so the trigger is not identified; whether the 2026-09-05 pass ran under it is unknown, because block screencaps are not committed artifacts. **A re-run of this device with a verified keyboard-free foreground is owed.**
2. **frust S8 graded `DEGENERATE` twice by a stale driver check; the data is complete.** `matrix.sh`'s `block_sanity` counts lines matching `-perf plugin op=`, but Frust's S8 emitters now carry the canonical inline `scenario=` key first (`frust-perf plugin scenario=s8-write op=write type=bool n=3 us=102 err=0`), which PROTOCOL §7 documents as shipped since `198d3eb8`; only the two `op=…type=total` roll-up lines still match the old pattern, so the check saw 2 where the run captured 2,002 per-op lines — the same count as the 2026-09-05 series. `stats.py` parses the new shape correctly and the S8 table above is computed from it. The block was re-run once, graded the same way, and `pick_src` fell back to attempt 1; the staged tree holds the full series. The one-line driver-pattern fix is owed.
3. **One thermal cooldown timeout.** The frust S6 block opened at 38.2 °C and the 120 s wait did not clear the 38 °C ceiling (`warning: still 38.2C after 120s wait — timed out, proceeding anyway`), making that block 506 s of wall against 384 s for the others. It is the block immediately after the session's peak (the flutter S5 block ended at 38.2 °C); no other block stalled and the session ended at 32.6 °C.
4. **S7 idle CPU is 0 samples on both sides.** Neither package appeared in the `dumpsys cpuinfo` samples taken every 30 s through the S7 blocks, so the row reads ~0 % for both apps rather than carrying a number — the same reading the 2026-09-05 pass arrived at after discounting its contaminated samples.
5. **APK packaging differs between the two apps this pass** — the Frust APK is arm64-only (12,008,167 B) and the Flutter APK a fat profile build (70,462,089 B). Both were installed fresh with md5 re-verified; neither figure is a size comparison, which is the App size section's job.
6. **Flutter S2 rows are not comparable to any Flutter S2 row before 2026-09-05** — the Flutter bench app was changed to single-line ellipsis for row-text parity with Frust, so its S2 series measures different work than the earlier builds did.
7. **This device's S5 rows supersede the 2026-09-05 S5 row, which is invalid** — that capture was taken while a full atlas refused image draws outright, so roughly half the image composites it timed were never performed (PROTOCOL §7's note on the `img` line). The row above is the first valid S5 measurement on this device.
8. As on prior passes: Flutter release-mode in-app cross-check not captured; S4 parse wall time not recoverable (timestamp-free capture); S8 burst-during-animation variant not run.

---

## Device: iPhone SE (2nd gen) (iPhone12,8 / Apple A13 Bionic) — iOS, 60 Hz budget tier

**Status:** run 2026-09-06 15:32–17:54 UTC, **full-form: 12 runs × 30 s per scenario per app (S7 also 30 s — the 60 s length only serves Android's idle window), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** Frust = **PROFILE** build of `21076221` (engine renderer with the render-thread split; `frust build … --profile --define FRUST_TRACE_RAW=1`, raw format v4); Flutter = profile build (`flutter build … --profile`, Flutter 3.47.2), one `flutter_sN.app` installed per scenario. Raw series under `raw/iphone_se/frust_profile/<sN>/` and `raw/iphone_se/flutter/<sN>/` (192 run logs, 12 per scenario per app; no PSS snapshots on iOS). All 16 blocks captured in one unattended session; two were re-run (deviations 1–2). **This pass supersedes the 2026-09-05 pass on this device.**

- Chipset: Apple A13 Bionic (arm64e); iPhone SE 2nd gen (iPhone12,8); serial/UDID redacted.
- OS: iOS 26.6.1, Developer Mode on, network-paired `devicectl` transport (the device reported `available (paired)` throughout).
- Display: 60 Hz panel, single mode. **The 8.33 ms column is N/A on this device** (summarize.py still prints it; it is not discussed).
- Refresh opt-in (§6): no >60 Hz mode exists, so the gate is moot; Frust ran ~61.4 fps and Flutter ~67–68 fps in every frame scenario.
- Brightness / airplane / Wi-Fi / BT: **uncontrolled** (no iOS CLI). Charger: USB-connected throughout (devicectl requirement).
- Thermal: no sensor CLI; fixed 60 s inter-block cooldown (`--cooldown 60`); no gate stalls possible.
- Toolchain: Xcode 26.2 (17C52), rustc 1.98.1 (pinned), `wgpu` 30.0.1 (pinned), Flutter 3.47.2 stable / Dart 3.13.2.
- Visual gate: **skipped** (no iOS screenshot CLI). Driver marker/line-count sanity passed on 14 of 16 blocks at the first attempt (frust blocks 380–385 s wall, Flutter 450–456 s incl. per-scenario install + container pull).
- Sanitization: driver staging `RAW_OK`; whitelist self-check passed — 0 lines outside the whitelist in all 192 logs.
- iOS conventions (unchanged): Frust `total_us` (rows marked †) folds in the CADisplayLink acquire/vsync wait (`acquire` p50 10.9–13.2 ms), so † totals pin to the 16.67 ms cadence; **the "CPU work" row is `total − acquire` = rebuild+layout+paint+encode+submit** and is the per-frame comparison row. Flutter's `totalSpan` goes negative under load, so its frame total is `build_us+raster_us`. **For the engine, `submit` waits on the GPU, so that row is CPU+GPU on the Frust side while Flutter's `build+raster` is CPU only — the two are not like-for-like, and no per-frame winner is declared from them; Frust's real GPU time is the `gpu_total` figure in each pass-breakdown line (Metal timestamps, `gpu_q=1`).**

### S1

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 16.71 | 18.20 | 18.54 | 42.16 | 9,729 (52.84%) | 18,302 (99.41%) | 18,411 (~61.4 fps avg) |
| Frust CPU work (total − acquire wait) | 5.79 | 6.47 | 6.90 | 10.62 | 0 (0.00%) | 24 (0.13%) | 18,411 (~61.4 fps avg) |
| Flutter (profile) | 5.10 | 5.53 | 5.68 | 20.78 | 6 (0.03%) | 12 (0.06%) | 20,092 (~67.0 fps avg) |

Frust pass p50 (ms): rebuild 0.03, layout 0.01, paint 0.05, encode 0.05, acquire 10.94, submit 5.61; layout_us>0 on 18,411/18,411 frames; GPU (gpu_q=1 on 18,371) total p50/p95 4.38/4.58 ms


**Flutter carries less CPU work per frame; both hold the panel.** On the comparison row Frust is 5.79 ms against Flutter's 5.10 (p95 6.47 vs 5.53), and neither app misses the 60 Hz budget on the work axis (0.00 % both). Frust's GPU pass time is 4.38 ms median, so most of the 5.79 ms is GPU wait inside `submit` rather than CPU — see the iOS conventions above before reading the two rows as like-for-like.
### S2

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 19.28 | 21.00 | 21.79 | 24.93 | 14,570 (79.29%) | 15,153 (82.46%) | 18,376 (~61.3 fps avg) |
| Frust CPU work (total − acquire wait) | 6.47 | 7.50 | 7.99 | 11.06 | 0 (0.00%) | 53 (0.29%) | 18,376 (~61.3 fps avg) |
| Flutter (profile) | 4.93 | 5.65 | 5.97 | 10.55 | 0 (0.00%) | 11 (0.17%) | 6,423 (~21.4 fps avg) |

Frust pass p50 (ms): rebuild 0.10, layout 2.53, paint 0.15, encode 0.03, acquire 13.18, submit 3.00; layout_us>0 on 18,376/18,376 frames; GPU (gpu_q=1 on 18,336) total p50/p95 3.08/3.14 ms


**Frust pays more per frame and paints far more of them.** Work row 6.47 vs 4.93 ms; Frust repaints continuously at ~61.3 fps (18,376 frames) while Flutter's scroll paint is event-driven (6,423 frames), so the per-frame figures compare and the frame counts do not. Frust re-lays out every frame at 2.53 ms median, which is the bulk of its cost here.
### S4

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 16.60 | 17.09 | 17.65 | 39.42 | 7,789 (42.23%) | 13,188 (71.50%) | 18,446 (~61.5 fps avg) |
| Frust CPU work (total − acquire wait) | 3.69 | 4.39 | 4.60 | 8.83 | 0 (0.00%) | 1 (0.01%) | 18,446 (~61.5 fps avg) |
| Flutter (profile) | 1.38 | 1.61 | 1.73 | 7.76 | 0 (0.00%) | 0 (0.00%) | 20,335 (~67.8 fps avg) |

Frust pass p50 (ms): rebuild 0.11, layout 0.02, paint 0.04, encode 0.02, acquire 12.70, submit 3.48; layout_us>0 on 18,446/18,446 frames; GPU (gpu_q=1 on 18,406) total p50/p95 0.62/0.84 ms


**Flutter wins S4 on the work axis** — 1.38 vs 3.69 ms p50, 1.61 vs 4.39 p95 — with neither app missing the 60 Hz budget. Frust's GPU time is 0.62 ms, the cheapest of the matrix, so this row is CPU-side scheduling under the concurrent animation, not rasterisation.
### S5

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 16.90 | 17.86 | 18.25 | 39.85 | 11,043 (60.00%) | 17,940 (97.47%) | 18,406 (~61.4 fps avg) |
| Frust CPU work (total − acquire wait) | 5.47 | 6.11 | 6.42 | 10.31 | 0 (0.00%) | 12 (0.07%) | 18,406 (~61.4 fps avg) |
| Flutter (profile) | 2.62 | 3.60 | 3.74 | 6.60 | 0 (0.00%) | 0 (0.00%) | 20,228 (~67.4 fps avg) |

Frust pass p50 (ms): rebuild 0.50, layout 0.03, paint 0.03, encode 0.01, acquire 11.51, submit 4.89; layout_us>0 on 18,406/18,406 frames; GPU (gpu_q=1 on 18,366) total p50/p95 4.64/4.73 ms


**Flutter wins the work axis; Frust's image column is complete.** Work row 5.47 vs 2.62 ms. As on the OnePlus 9, this capture composites the whole column: 16,941 `frust-perf img` lines across the kept runs, `skipped=0` on every one, 54,095 evictions and up to 60 resident images against the atlas budget — PROTOCOL §7's content gate is met with eviction-only degradation.
### S6

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 17.07 | 17.85 | 18.36 | 42.58 | 15,916 (86.01%) | 18,396 (99.41%) | 18,505 (~61.7 fps avg) |
| Frust CPU work (total − acquire wait) | 5.57 | 6.28 | 6.64 | 13.80 | 0 (0.00%) | 12 (0.06%) | 18,505 (~61.7 fps avg) |
| Flutter (profile) | 4.30 | 5.14 | 5.38 | 15.75 | 0 (0.00%) | 17 (0.08%) | 20,498 (~68.3 fps avg) |

Frust pass p50 (ms): rebuild 0.07, layout 0.16, paint 0.25, encode 0.08, acquire 11.50, submit 5.02; layout_us>0 on 18,505/18,505 frames; GPU (gpu_q=1 on 18,465) total p50/p95 1.64/1.86 ms


**Closest scenario on this device** — work row 5.57 vs 4.30 ms p50, 6.28 vs 5.14 p95, neither app missing the 60 Hz budget. Frust's GPU time is 1.64 ms; the cost is shaping and glyph upload on the CPU side.
### S3

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | 26.41 / 27.31 / 31.66 / 31.79 | 180 | **not captured** | 0 |
| create 10k | 24.32 / 25.05 / 25.32 / 25.48 | 190 | **not captured** | 0 |
| update every 10th of 10k | 20.83 / 21.63 / 22.04 / 22.20 | 185 | **not captured** | 0 |
| swap | 19.89 / 21.82 / 22.14 / 22.60 | 180 | **not captured** | 0 |
| clear | 19.17 / 19.94 / 20.31 / 20.60 | 180 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 16.73 | 18.48 | 24.69 | 31.79 | 10,144 (55.11%) | 16,779 (91.15%) | 18,408 (~61.4 fps avg) |
| Flutter (profile) | 5.05 | 12.71 | 13.54 | 16.86 | 3 (0.29%) | 315 (30.23%) | 1,042 (~3.5 fps avg) |

Cycle health — Frust (profile) `s3-create1k` reopens per kept run: 19–19; Flutter (profile) `s3-create1k` reopens per kept run: 15–22.


**All five Frust reconcile ops are captured, and their numbers are not comparable with 2026-09-05's.** The frame-indexed half-open windows attribute exactly the op's own frame, where the previous pass sliced by log position and attributed a neighbouring frame to each op (PROTOCOL §7's marker-format note). That is why the per-op p50s read 19.2–26.4 ms here against 15.7–16.8 ms before while **the overall series is flat** — p50 16.79→16.73 ms over a near-identical 18,276→18,408 frames, and its p95 and 60 Hz miss rate both improved. Read the change as a correction of attribution, not a regression in reconcile cost. Flutter still lands no per-op frames at all on this device.
### S7

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | not captured | not captured |
| Framework-reported first-frame span | ~100 ms median (`first_frame_presented`, 64–176 across 12 launches; `adapter_ready` median 22 ms) | ~8 ms median (`first_frame_ms`, 6–9 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | n/a | n/a |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | n/a | n/a |


**Frust's GPU bring-up got much faster; its first frame did not follow.** `adapter_ready` median falls 102→22 ms (max 239→34), but the first-frame span only moves 110→100 ms because it now waits on the font preinit instead: across the 12 launches `first_frame_presented` equals `font_preinit_joined` plus 13 ms ±1 in every single one, and the join itself is bimodal — six launches at 51–65 ms (first frame 64–82 ms) and six at 100–159 ms (first frame 119–176 ms). The ≤ 85 ms criterion is missed on the median and met on exactly the fast half; the criteria section states it plainly. Flutter's ~8 ms figure is its own framework-entry span and bounds different work.
### S8

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (profile) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 4 | 132 | n/a |
| write i64 | 3 | 56 | n/a |
| write f64 | 4 | 55 | n/a |
| write String | 4 | 52 | n/a |
| write Vec\<String\> | 5 | 61 | n/a |
| read (unique key, forces channel) bool | 1 | 2692† | ~0 |
| read (unique key, forces channel) i64 | 1 | 2692† | ~0 |
| read (unique key, forces channel) f64 | 1 | 2692† | ~0 |
| read (unique key, forces channel) String | 1 | 2692† | ~0 |
| read (unique key, forces channel) Vec\<String\> | 1 | 2692† | ~0 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust wins S8 by a wide margin on iOS** — writes 3–5 vs 52–132 µs/call, and a per-key read at 1 µs against Flutter's whole-store `reload()` at 2,692 µs. Both sides are cheaper here than on Android because the iOS preferences store is in-process on both. Zero `s8-errors` on either app.

### vs 2026-09-05 (Frust CPU-work row, same device, same scenarios)

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new (ms) | Δp95 | miss@16.67 old→new |
|---|---|---|---|---|---|
| S1 | 5.79→5.79 | +0.1% | 6.48→6.47 | -0.0% | 0.00%→0.00% |
| S2 | 6.34→6.47 | +2.0% | 7.33→7.50 | +2.3% | 0.00%→0.00% |
| S4 | 3.42→3.69 | +7.9% | 4.03→4.39 | +8.8% | 0.00%→0.00% |
| S5 | 5.31→5.47 | +3.1% | 6.58→6.11 | -7.2% | 0.00%→0.00% |
| S6 | 5.54→5.57 | +0.5% | 6.28→6.28 | -0.1% | 0.00%→0.00% |
| S3 (overall) | 16.79→16.73 | -0.3% | 18.78→18.48 | -1.6% | 62.32%→55.11% |

The per-frame work row is unchanged within noise on four of six scenarios (−0.0…+3.1 %); S4 is the one outlier at +7.9 % p50 / +8.8 % p95, and the Flutter control moved +3.4 % / +6.2 % on the same scenario in the same session, so most of that is session-level rather than Frust-side. The Flutter control moved −4.3…+3.8 % across the board. Off the frame axis the pass is clearly better: `adapter_ready` 102→22 ms median, first-frame span 110→100 ms (min 78→64, max 250→176), and the S3 overall series holds its p50 while dropping p95 18.78→18.48 ms and its 60 Hz miss rate 62.32→55.11 %. The S3 **per-op** rows are the one place a straight old-vs-new read is wrong — the marker attribution changed under them; see the S3 note above.

### Methodology deviations (this device)

1. **flutter S3 re-run once (Flutter-side timing delivery).** On the first attempt two of the twelve runs under-delivered frame timings — run-09 produced 0 `flutter-perf raw` lines and run-05 produced 26 against ~107 in every other run — which is the known Flutter iOS `addTimingsCallback` delivery gap, not a capture fault. The retry graded `ok` (kept line counts 76–108) and is the staged series.
2. **frust S8 graded `DEGENERATE` twice by a stale driver check; the data is complete.** Identical to the OnePlus 9 case: `matrix.sh`'s `block_sanity` counts `-perf plugin op=` while Frust's S8 emitters now lead with the canonical inline `scenario=` key, so the check saw 2 lines where the run captured 2,002 per-op lines. `stats.py` parses the new shape and the S8 table above is computed from it; `pick_src` fell back to attempt 1. The one-line driver-pattern fix is owed.
3. **No visual gate and no thermal or battery telemetry on this device** — iOS exposes no screenshot, temperature or battery-level CLI to the driver, so this device's block sanity is marker/line-count only and its thermal control is the fixed 60 s inter-block cooldown.
4. **Flutter's fps column is not a presented rate here.** Flutter reports 20,092–20,498 frames per 300 s capture (~67–68 fps) on a 60 Hz panel, so its timing callbacks are not one-per-present; Frust's ~61.4 fps is a presented count. Compare the two apps on the per-frame rows, not the fps column.
5. **Flutter S2 rows are not comparable to any Flutter S2 row before 2026-09-05** — the Flutter bench app was changed to single-line ellipsis for row-text parity with Frust.
6. As on prior passes: no external `am start -W` equivalent is captured on iOS (the S7 cold-start row reads "not captured" for both apps), no PSS/idle-memory axis exists, Flutter release-mode in-app cross-check not captured, and the S8 burst-during-animation variant was not run.

---

## Device: Xiaomi 12 (cupid, Snapdragon 8 Gen 1 / Adreno 730) — headline-tier Android — **vello-era pass, pending re-run**

**Status:** run 2026-07-21 on the **vello renderer (retired 2026-09-02, PROTOCOL §2.6)** — the
device was not at desk for the 2026-09-05 engine pass, so this section stands as the only
headline-tier data point until it is re-run. 12 runs × 30 s per scenario per app (S7 60 s),
first 2 discarded. **Frust = RELEASE build of `a55e09e` (pre-§2.5 methodology, instrumentation
compiled in via the `perf.rs` touch recipe); Flutter 3.44.2 profile.** Raw series (v2 format)
committed under `raw/xiaomi12/frust_release/<sN>/` and `raw/xiaomi12/flutter/<sN>/`. Tables
regenerated from those raws with `harness/summarize.py` on 2026-09-05; the original narrative
section is in git history (`git show f64be636:benchmarks/RESULTS.md`).

- Chipset: Snapdragon 8 Gen 1 (SM8450) / Adreno 730; model 2201123G (cupid), serial redacted
- OS: LineageOS 23.2 (Android 16, userdebug) — not stock MIUI/HyperOS
- Display: 1080×2400, 120 Hz mode active for the whole session; both budgets reported. High-refresh
  opt-in on both apps (Frust `Surface.setFrameRate`, Flutter `flutter_displaymode`)
- Controls: brightness 128 fixed, airplane on, Wi-Fi/BT off, `dumpsys battery unplug` spoof per
  block, thermal gate ≤38 °C never stalled (35.1 → 32.5 °C), visual gate passed on every scenario
  incl. S5 edge-to-edge parity, all 192 logs whitelist-clean

### S1

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | 4.87 | 5.77 | 6.72 | 63.52 | 35 (0.10%) | 212 (0.59%) | 35,902 (~119.7 fps avg) |
| Flutter (profile) | 9.03 | 18.59 | 22.48 | 38.63 | 1,759 (5.44%) | 26,699 (82.62%) | 32,315 (~107.7 fps avg) |


**Frust wins S1 decisively** — 4.9 ms median at a locked ~120 fps vs Flutter's 9.0 ms and 5.4 % of frames past the 60 Hz budget. Neither field settled within any 30 s window (0 skipped frames on both).

### S2

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | 10.35 | 16.40 | 18.96 | 52.60 | 825 (4.23%) | 14,390 (73.73%) | 19,518 (~65.1 fps avg) |
| Flutter (profile) | 13.57 | 23.21 | 26.68 | 38.28 | 1,605 (24.85%) | 5,186 (80.28%) | 6,460 (~21.5 fps avg) |


**Frust wins S2** — median 10.4 vs 13.6 ms, p95 16.4 vs 23.2 ms, ~6× lower 60 Hz-miss rate. Flutter's smaller absolute counts reflect its event-driven paint (~3× fewer frames), not better per-frame cost.

### S3

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | 7.25 / 8.64 / 48.70 / 50.03 | 326 | **not captured** | 0 |
| create 10k | 13.62 / 21.52 / 23.84 / 25.36 | 325 | **not captured** | 0 |
| update every 10th of 10k | 10.90 / 13.53 / 18.99 / 20.18 | 324 | **not captured** | 0 |
| swap | 10.03 / 11.55 / 16.71 / 19.60 | 321 | **not captured** | 0 |
| clear | 11.41 / 12.81 / 20.14 / 23.26 | 320 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | 4.87 | 9.99 | 13.17 | 50.03 | 85 (0.26%) | 3,018 (9.36%) | 32,255 (~107.5 fps avg) |
| Flutter (profile) | 13.81 | 30.07 | 34.64 | 39.58 | 327 (35.47%) | 784 (85.03%) | 922 (~3.1 fps avg) |

Cycle health — Frust (release) `s3-create1k` reopens per kept run: 32–33; Flutter (profile) `s3-create1k` reopens per kept run: 19–19.

Frust's per-op reconcile frames all sit inside one 60 Hz frame except create-10k (13.6 ms); **Flutter's per-op capture landed zero frames in every sub-marker window** (the `addTimingsCallback` async-delivery gap seen on every device), so no cross-app per-op comparison exists. No overall winner is declared from the whole-series rows (continuous vs event-driven paint).

### S4

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | 4.50 | 5.49 | 6.08 | 59.36 | 12 (0.03%) | 41 (0.11%) | 36,150 (~120.5 fps avg) |
| Flutter (profile) | 3.64 | 5.41 | 5.72 | 15.99 | 0 (0.00%) | 16 (0.04%) | 35,999 (~120.0 fps avg) |


**Roughly a tie, Flutter marginally smoother** — both hold ~120 fps while the parse runs off the UI thread; Flutter's tail is tighter (worst 16.0 vs 59.4 ms).

### S5

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | 5.72 | 12.29 | 13.42 | 53.16 | 35 (0.13%) | 7,968 (29.99%) | 26,572 (~88.6 fps avg) |
| Flutter (profile) | 5.29 | 9.29 | 11.21 | 19.27 | 2 (0.01%) | 4,218 (12.17%) | 34,671 (~115.6 fps avg) |


**Flutter wins S5 narrowly at the median** (5.3 vs 5.7 ms) and clearly at the tail (p95 9.3 vs 12.3 ms); Frust settled at ~89 fps vs Flutter's ~116 fps under the full-width cell geometry.

### S6

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (release) | 6.21 | 11.20 | 12.58 | 50.63 | 50 (0.19%) | 8,081 (30.60%) | 26,410 (~88.0 fps avg) |
| Flutter (profile) | 7.43 | 9.72 | 10.69 | 13.35 | 0 (0.00%) | 9,724 (27.08%) | 35,914 (~119.7 fps avg) |


**Split — Frust wins the median (6.2 vs 7.4 ms), Flutter wins the tail** (p95 9.7 vs 11.2 ms, worst 13.4 vs 50.6 ms, zero 60 Hz misses).

### S7

| Metric | Frust (release) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | not captured | not captured |
| Framework-reported first-frame span | ~87 ms median (`first_frame_presented`, 82–104 across 12 launches; `adapter_ready` median 33 ms) | ~64 ms median (`first_frame_ms`, 49–71 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | n/a | n/a |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~53.7 MB (53.4–54.0, 10 kept snapshots) | ~89.6 MB (86.3–90.7, 10 kept snapshots) |

**Frust wins S7 on memory** (~54 vs ~90 MB idle PSS); Flutter's self-reported first frame is earlier (64 vs 87 ms). External cold start was measured post-matrix in the original pass (Frust ~150 ms vs Flutter ~300 ms class, see git history); idle CPU read 0 % on both.

### S8

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (release) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 97 | 288 | n/a |
| write i64 | 75 | 285 | n/a |
| write f64 | 70 | 300 | n/a |
| write String | 68 | 228 | n/a |
| write Vec\<String\> | 69 | 231 | n/a |
| read (unique key, forces channel) bool | 20 | 5133† | ~0 |
| read (unique key, forces channel) i64 | 19 | 5133† | ~0 |
| read (unique key, forces channel) f64 | 20 | 5133† | ~0 |
| read (unique key, forces channel) String | 20 | 5133† | ~0 |
| read (unique key, forces channel) Vec\<String\> | 20 | 5133† | ~0 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust wins S8 decisively** — ~3–4× on writes (68–97 vs 228–300 µs), and its per-key read (~20 µs) is a real backend call where Flutter's only channel-crossing read is a whole-store `reload()` (5.1 ms).

### Methodology deviations (this device)

1. Vello renderer, release build, pre-2026-07-23 methodology — not comparable with the engine-pass
   sections above without the §2.5/§2.6 bounds; kept only because no engine pass exists yet.
2. Flutter S3 per-op capture landed zero frames (structural `addTimingsCallback` gap).
3. S4 parse wall-time not recoverable (timestamp-free capture); S8 burst-during-animation variant
   and Flutter release-mode cross-check not run.

---

## App size (release) — Frust re-measured 2026-09-06, Flutter 2026-09-05

Frust's rows are release builds of `21076221`; the Flutter column is the
2026-09-05 `app_size.sh` snapshot of Flutter 3.47.2 (`flutter build apk
--release [--split-per-abi]` / `flutter build ios --release`), unchanged this
pass and dated accordingly. The Frust Android release is signed with a
throwaway local keystore purely to satisfy the CLI's release-signing gate;
signing does not change the size class. Sizes are MiB alongside the exact byte
count, which is the authoritative figure.

| Axis | Frust, `db` on (default) | Frust, `db` off (`--no-default-features --features lean`) | Flutter (2026-09-05) |
|---|---|---|---|
| Android universal release APK (3 ABIs) | **31.47 MB** (33,002,246 B) | not built as universal | 49.36 MB (51,762,140 B) |
| Android arm64-v8a split APK | **10.95 MB** (11,480,155 B) | **9.09 MB** (9,533,659 B) | 17.45 MB (18,295,458 B) |
| in-APK `lib/arm64-v8a/libfrustbench.so` | 10.83 MB (11,352,480 B) | 8.97 MB (9,405,968 B) | 16.55 MB (engine + app) |
| on-disk arm64 `.so` (stripped) | 10.83 MB (11,352,488 B) | 8.97 MB (9,405,976 B) | n/a |
| iOS release `.app` (`du -sk`) | **11.63 MB** (11,908 KB) | not built | 16.64 MB |
| iOS `Runner` binary | 11.54 MB (12,103,152 B) | not built | n/a |

Two findings from this measurement, both about where the bytes are:

- **Routing `wgpu` per target saves ~0 bytes.** Restricting each target to the
  backends and `naga` writers it can actually use (Android → `vulkan`/`spv-out`,
  Apple → `metal`/`msl-out`, Windows → `dx12`+`vulkan`/`hlsl-out`+`spv-out`,
  from `dx12`+`metal`+`vulkan` everywhere) moves the universal APK by 0 bytes
  and the arm64 `.so` by +16 B. Fat LTO with `codegen-units = 1` had already
  dead-stripped the unreferenced writers: per-crate attribution of the
  unstripped arm64 `.so` is byte-identical across the change — `naga` 890,515 B
  both ways, attributed total 7,406,938 → 7,406,834 B. The change is build
  hygiene and correctness of intent, not a size lever.
- **The `db` feature gate is the lever.** Turning the DB scenarios off drops the
  arm64 `.so` from 11,352,488 to 9,405,976 B (−1,946,512 B, −17.1 %) and the
  arm64 split APK by the same −1,946,496 B (−17.0 %). Attributed symbol bytes
  fall 7,406,834 → 5,722,214; the rows that disappear are bundled `sqlite3`
  (671,557 B), its unmangled C helpers (951,967 B) and `rusqlite`. What remains
  on top is led by `naga` 890,515, `core` 660,128, `wgpu_core` 550,868,
  `harfrust` 387,678 and `skrifa` 288,148 — so the next size question is the
  shader-translation and GPU-plumbing tier, not SQLite.

Frust stays the smaller artifact on every axis it shares with Flutter. Against
the 2026-07-21 vello-era snapshot it is still larger (universal 21.25 → 31.47
MB, arm64 `.so` 7.42 → 10.83 MB, iOS 7.96 → 11.63 MB), and the bench app itself
grew between the two snapshots because the `d1`/`d2` scenarios pull in
`frust-database` with a bundled SQLite — the `db`-off column above is the size
of that difference, and the remainder is the open renderer-attribution question.

## Renderer transition (vello → frust-engine) — regression check, 2026-09-05

Recorded once, on the 2026-09-05 pass: `harness/compare.py` over two
`summarize.py` JSON dumps, the retired vello series (git history,
`git show f64be636:benchmarks/raw/…`) against that pass's raws; Δ negative =
faster. The same-build-mode (`--profile`) vello rows were the renderer
comparison proper, the 2026-07-21 release rows cross-methodology (PROTOCOL
§2.5, tails only). The full per-device comparison tables were folded into this
summary on 2026-09-06, when the post-optimization pass above replaced the
device sections they were computed against; they remain in git history
(`git show de265855:benchmarks/RESULTS.md`).

**Verdict: no per-frame renderer regression on Android or iOS in the
like-for-like rows.** Its two open measurement caveats — S5 content on the
OnePlus 9 and S3 per-op attribution — are settled by the 2026-09-06 pass above;
its iOS tail items are carried into that pass's criteria section.

- **Same build mode, the renderer comparison proper** (vello `--profile`
  2026-09-01 → engine `--profile` 2026-09-05). OnePlus 9: S1 p50 14.84→8.20 ms
  (−45 %), S2 17.99→8.83 (−51 %), S5 22.07→7.78 (−65 %), S6 flat, every cell
  moving from 47–68 fps to a locked 120 fps. Pixel 5 (unpinned): the largest
  gains of that matrix — S1 48.40→14.86 (−69 %), S2 42.96→8.91 (−79 %), S5
  95.56→13.19 (−86 %), S6 23.23→6.82 (−71 %), with every vello cell missing the
  60 Hz budget on 100 % of frames against the engine's 0.1–1.7 %. iPhone SE: the
  60 Hz cadence identical, per-frame cost fully observable at CPU 0.14–2.7 ms
  plus GPU 4.4 / 3.1 / 1.6 ms for S1 / S2 / S6; its "CPU work" row rose
  +8…+22 % only because the engine's `submit_us` waits on the GPU inside it
  while vello's compute was asynchronous and never measured — bookkeeping.
  Engine-to-engine drift was −3…−10 % (OnePlus 9) and within ±5 % (Pixel 5).
- **Cross-methodology** (vello RELEASE 2026-07-21 → engine profile, OnePlus 9):
  S1/S2/S5 tails −22…−56 %, S3 per-op −10…−22 %, cold start 185→135 ms, first
  frame 128→105 ms. The two rows that moved the wrong way are §2.5 artefacts:
  S6 p50 5.67→8.19 ms is the 120 Hz pin (period 8.33 ms; the same-build row is
  flat) and S8 writes 65–67→94–95 µs are plugin calls, not renderer work.
  Flutter's own 3.44.2→3.47.2 drift stayed <7 %, so the cross-app verdicts are
  not a Flutter artefact.

## DB scenarios (`d1`/`d2`) — no runs recorded yet

`PROTOCOL.md` §9 specifies the `d*` op-latency class (`d1` batched/autocommit
writes, `d2` point SELECT + range scan) against the fixed row shape and seed
dataset of §9.2, on §7's per-op raw-line contract. **No device has run it.**
When a pass is captured this section gains one subsection per device with a
`d1` and a `d2` table (per-op p50/p95/p99 µs + ops/s) across three columns —
Frust `frust-database` (in-process `rusqlite`), Flutter `package:sqlite3`
(engine-parity FFI) and Flutter `sqflite` (platform channel) — each side's
linked SQLite version recorded per run, plus a methodology-deviations list.
