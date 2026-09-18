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
device's delta table ("vs 2026-09-05"; "vs 2026-07-21" on the Xiaomi 12) is `harness/compare.py` over two of those JSON
dumps, both regenerated from the committed raw trees; Δ positive = slower. Prose mentions of an
earlier pass's own idle-memory figure are likewise recomputed from that pass's raw series in git
history, not copied from its published text (`de265855` Pixel 5, `32597904` OnePlus 9, `15559808`
Xiaomi 12).

> **Post-optimization pass, 2026-09-06.** Every Frust series below is a
> `--profile` build of `21076221` on the frust-owned `frust-engine` strip
> pipeline with the render-thread split. On the OnePlus 9, iPhone SE and Pixel 5
> it replaces that device's 2026-09-05 series (PROTOCOL §2.6) and each section
> carries a compact delta table against it; on the Xiaomi 12 it is the first
> engine-renderer pass, replacing the 2026-07-21 vello-era series, and its
> section carries a renderer-transition comparison instead. **All four devices
> ran the full 16 blocks**, in one unattended `matrix.sh` session each: OnePlus 9
> (USB, 120 Hz pinned), iPhone SE (network `devicectl`), Pixel 5 (adb-over-Wi-Fi,
> unpinned), Xiaomi 12 (USB, 120 Hz pinned, 21:13–23:14 UTC after the other
> three). Frust's `vello` renderer was deleted on 2026-09-02 and the 2026-07-21
> vello-era matrix retired with it.
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

**iPhone SE first frame — missed on the median, met on half the launches.** 100 ms median against the 85 ms bar, better than 2026-09-05's 110 ms on every statistic (min 78→64, max 250→176). The distribution is bimodal — six launches at 64–82 ms, six at 119–176 — and the split is entirely the font preinit: `first_frame_presented` = `font_preinit_joined` + 13–23 ms (median 17) in all twelve launches, the join being 51–65 ms in the fast half and 100–159 in the slow. GPU bring-up is no longer the constraint (`adapter_ready` 102→22 ms median, max 239→34), so closing this is a font-preinit question, not a renderer one.

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
- Charger: **AC-connected at the start, disconnected mid-session** — `AC powered: true` at 100 % when the session opened, the level flat at 100 % through the frust S5 block (18:54 UTC), then 98 % → 91 % over the remaining blocks (flutter S5, S6, S7, S8) with `AC powered: false` at close; so S1–S4 and frust S5 ran on mains and the rest on battery. `dumpsys battery unplug` spoofed on-battery state before every block either way. **The 2026-09-05 session ran on battery throughout (99 % → 78 %)** — see the delta note below, this is a real environmental difference between the two passes.
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
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~94.2 MiB (93.8–94.6, 10 kept snapshots) | ~118.1 MiB (115.2–126.0, 10 kept snapshots) |

**Frust starts ~4× faster and idles ~20 % lighter.** External cold start 159 vs 638 ms (`am start -W` TotalTime), idle TOTAL PSS 94.2 vs 118.1 MiB. The framework first-frame spans are close on this device (172 vs 162 ms median) and, as always, are measured from each framework's own entry point. Neither app appeared in `dumpsys cpuinfo` during the idle window, i.e. idle CPU ~0 % for both.

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
| S3 (overall) | 5.97→6.61 | +10.7% | 9.60→10.84 | +12.9% | 0.16%→0.53% | n/a→n/a |

**Read this table with the control beside it, not on its own.** Frust moved +2.5…+15.7 % on p50 (S5 −1.1 %), but the **Flutter control on the same runs moved −39.5 % to +38.1 %** — S2 p50 22.65→31.28 (+38.1 %), S4 p95 18.55→6.74 (−63.7 %), S6 p50 16.94→10.25 (−39.5 %), S5 p50 6.27→8.39 (+33.9 %). An unchanged control app swinging by up to 64 % between the two sessions means this device's session-to-session variance is several times the Frust delta, so **no per-scenario conclusion should be drawn from the Frust column here**. Two environmental differences are documented and plausibly responsible: the 2026-09-05 session ran **on battery** (99 → 78 %) while this one ran **on AC for S1–S4 and frust S5 and on battery from flutter S5 onward** (100 → 91 %, charger disconnected mid-session), and the panel idled at **90 Hz** this session against 60 Hz last time — on an unpinned device both feed directly into the DVFS and refresh policy the frame loop runs under. Off the frame axis, where the metrics are not per-frame: external cold start 136→159 ms is worse, and idle TOTAL PSS rose 88.5→94.2 MiB (Frust) and 110.3→118.1 MiB (Flutter) on both sides, so it is not a Frust-side change. A clean re-measurement of this device with the charger state matched to the baseline is the way to settle the frame rows.

### Methodology deviations (this device)

1. **adb-over-Wi-Fi, radios left as found.** The device was reached over `--wireless`, so airplane mode, Wi-Fi and Bluetooth were not touched (toggling them would cut the link) and the §3 radios-off state was not applied. Same deviation as this device's 2026-09-05 pass.
2. **frust S8 graded `DEGENERATE` twice by a stale driver check; the data is complete.** Identical to the other two devices: `matrix.sh`'s `block_sanity` counts `-perf plugin op=` while Frust's S8 emitters now lead with the canonical inline `scenario=` key, so the check saw 2 lines where the run captured 2,002 per-op lines. `stats.py` parses the new shape and the S8 table above is computed from it; `pick_src` fell back to attempt 1. The driver check was corrected the same day (`block_sanity` and `run.sh`'s op counts now accept the `scenario=` shape, `7ee376b3`); a 12-run S8 block per app re-captured on the OnePlus 9 with the corrected check graded `ok` at 2,002 lines and reproduced that device's published S8 medians within 2 %, so the series above stand.
3. **The 90 Hz budget is not a column.** stats.py reports the 16.67 and 8.33 ms budgets only, so on this unpinned 60/90 Hz panel neither column is the frame period the apps were actually running to (11.11 ms at 90 Hz). Read the achieved-fps figures for cadence.
4. **Charger state differs from the 2026-09-05 baseline and changed mid-session** — mains for S1–S4 and frust S5, battery from flutter S5 onward (100 % → 91 %, `AC powered: false` at close), battery-only there. Recorded above because it bounds the delta table, not because it invalidates this pass's own Frust-vs-Flutter comparison, where both apps shared the identical session.
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
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~114.1 MiB (113.8–114.5, 10 kept snapshots) | ~153.7 MiB (153.4–154.0, 10 kept snapshots) |

**Frust starts ~3.3× faster and idles ~26 % lighter.** External cold start 128 vs 422 ms (`am start -W` TotalTime), idle TOTAL PSS 114.1 vs 153.7 MiB. Flutter reports the shorter framework first-frame span (79 vs 95 ms median), measured from each framework's own entry point, so the two spans bound different work; the external `am start` figure is the one measured identically on both sides. Neither app appeared in `dumpsys cpuinfo` during the idle window (0 samples both), i.e. idle CPU ~0 % for both.

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
| S3 (overall) | 5.57→5.66 | +1.7% | 12.16→12.26 | +0.8% | 2.14%→1.91% | n/a→n/a |

Frust's per-frame cost sum is 2–5 % higher on every scenario while the presented cadence is unchanged at ~120.1 fps, and the Flutter control moved −1.7…+2.3 % on the same runs, so the control is stable and the Frust move is real but small. Two caveats bound how far it can be read: only the frust blocks carried an extra composited system layer (deviation 1), and Frust's `acquire_us` — the swapchain wait, which that layer would lengthen — accounts for most of the difference (S1 1.28→1.61 ms, S5 1.12→1.32, S6 0.96→1.21). **S5 is not a regression at all**: it is +2.0 % while compositing an image column the 2026-09-05 capture dropped roughly half of. Off the frame axis the pass is faster: external cold start 137→128 ms, framework first frame 105→95 ms, `adapter_ready` 50→36 ms. Idle TOTAL PSS rose 102.2→114.1 MiB (Frust) and 139.0→153.7 MiB (Flutter), i.e. on both sides, so it is not a Frust-side change.

**The 8.33 ms column counts cost, not vsync misses** — under the render-thread split `total_us` is a two-thread cost sum, and this device's S1 is the worked example in the criteria section above (8.55 ms p50, 63.05 % "over budget", 120.1 fps presented).

### Methodology deviations (this device)

1. **Soft-keyboard window over the frust blocks only (composition asymmetry).** A Gboard window (`com.google.android.inputmethod.latin`) covers the lower third of all eight frust block screencaps and none of the eight flutter ones (bottom-band mean luma 237 vs 20–81 across the 16 captures). It did not change what either app drew: `dumpsys window` reports the bench window at the full `[0,0][1080,2400]` with `insetsChanged=false`, and the frust captures show content clipped mid-bubble and mid-cell at the keyboard edge rather than re-laid out, so both apps rendered their whole surface and every per-frame span measures the same scene. What it does add is one more layer for SurfaceFlinger to composite over the frust blocks, which is the most likely source of the `acquire_us` rise above. It therefore makes this device's Frust-vs-2026-09-05 deltas a soft comparison and its **Frust-vs-Flutter verdicts conservative for Frust**, since only Frust carried the extra layer. The condition did not reproduce after the session (`mInputShown=false`, both apps' post-run screencaps clean), so the trigger is not identified; whether the 2026-09-05 pass ran under it is unknown, because block screencaps are not committed artifacts. **A re-run of this device with a verified keyboard-free foreground is owed.**
2. **frust S8 graded `DEGENERATE` twice by a stale driver check; the data is complete.** `matrix.sh`'s `block_sanity` counts lines matching `-perf plugin op=`, but Frust's S8 emitters now carry the canonical inline `scenario=` key first (`frust-perf plugin scenario=s8-write op=write type=bool n=3 us=102 err=0`), which PROTOCOL §7 documents as shipped since `198d3eb8`; only the two `op=…type=total` roll-up lines still match the old pattern, so the check saw 2 where the run captured 2,002 per-op lines — the same count as the 2026-09-05 series. `stats.py` parses the new shape correctly and the S8 table above is computed from it. The block was re-run once, graded the same way, and `pick_src` fell back to attempt 1; the staged tree holds the full series. The driver check was corrected the same day (`block_sanity` and `run.sh`'s op counts now accept the `scenario=` shape, `7ee376b3`); a 12-run S8 block per app re-captured on this device at 19:17–19:30 UTC with the corrected check graded `ok` at 2,002 lines and reproduced the table above within 2 % (Frust writes 91–101 vs 92–99 µs, reads 19 vs 19; Flutter writes 282–310 vs 276–309 µs, `reload()` 4,467 vs 4,378), so the series above stand and the re-capture is not published.
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

**Frust's GPU bring-up got much faster; its first frame did not follow.** `adapter_ready` median falls 102→22 ms (max 239→34), but the first-frame span only moves 110→100 ms because it now waits on the font preinit instead: across the 12 launches `first_frame_presented` equals `font_preinit_joined` plus 13–23 ms (median 17) in every single one, and the join itself is bimodal — six launches at 51–65 ms (first frame 64–82 ms) and six at 100–159 ms (first frame 119–176 ms). The ≤ 85 ms criterion is missed on the median and met on exactly the fast half; the criteria section states it plainly. Flutter's ~8 ms figure is its own framework-entry span and bounds different work.

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
2. **frust S8 graded `DEGENERATE` twice by a stale driver check; the data is complete.** Identical to the OnePlus 9 case: `matrix.sh`'s `block_sanity` counts `-perf plugin op=` while Frust's S8 emitters now lead with the canonical inline `scenario=` key, so the check saw 2 lines where the run captured 2,002 per-op lines. `stats.py` parses the new shape and the S8 table above is computed from it; `pick_src` fell back to attempt 1. The driver check was corrected the same day (`block_sanity` and `run.sh`'s op counts now accept the `scenario=` shape, `7ee376b3`); a 12-run S8 block per app re-captured on the OnePlus 9 with the corrected check graded `ok` at 2,002 lines and reproduced that device's published S8 medians within 2 %, so the series above stand.
3. **No visual gate and no thermal or battery telemetry on this device** — iOS exposes no screenshot, temperature or battery-level CLI to the driver, so this device's block sanity is marker/line-count only and its thermal control is the fixed 60 s inter-block cooldown.
4. **Flutter's fps column is not a presented rate here.** Flutter reports 20,092–20,498 frames per 300 s capture (~67–68 fps) on a 60 Hz panel, so its timing callbacks are not one-per-present; Frust's ~61.4 fps is a presented count. Compare the two apps on the per-frame rows, not the fps column.
5. **Flutter S2 rows are not comparable to any Flutter S2 row before 2026-09-05** — the Flutter bench app was changed to single-line ellipsis for row-text parity with Frust.
6. As on prior passes: no external `am start -W` equivalent is captured on iOS (the S7 cold-start row reads "not captured" for both apps), no PSS/idle-memory axis exists, Flutter release-mode in-app cross-check not captured, and the S8 burst-during-animation variant was not run.

---

## Device: Xiaomi 12 (cupid, Snapdragon 8 Gen 1 / Adreno 730) — headline-tier Android

**Status:** run 2026-09-06 21:13–23:14 UTC, **12 runs × 30 s per scenario per app (S7: 60 s runs), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** **Frust = PROFILE build of `21076221`** (engine renderer with the render-thread split; `frust build … --profile --define FRUST_TRACE_RAW=1`, raw format v4 — the same binary the other three device sections ran), **Flutter = profile build** (`flutter build … --profile`, Flutter 3.47.2). Raw series under `raw/xiaomi12/frust_profile/<sN>/` and `raw/xiaomi12/flutter/<sN>/` (run-NN.log + run-NN.pss_before/after.txt, stats.txt; S7 adds cpuinfo.txt/coldstart.txt). One unattended `matrix.sh` session, all 16 blocks captured and graded `ok` on attempt 1. **This is the device's first engine-renderer pass; it replaces the 2026-07-21 vello-era release-build pass** (its `frust_release` raws left `benchmarks/raw/` with this pass — `git show 0b4b79c6:benchmarks/raw/xiaomi12/`; original narrative `git show f64be636:benchmarks/RESULTS.md`). Of the 2026-09-06 success criteria, the two with a device analogue hold here as well: S5 `skipped=0` over the whole column and `create 1k` 352 frames; S1 `gpu_main` reads 5.20 ms (the 5.0 ms bar was set for the OnePlus 9).

- Chipset: Snapdragon 8 Gen 1 (SM8450, board `taro`) / Adreno 730, GLES driver V@0615.95. Model: Xiaomi 12 2201123G (serial redacted).
- OS: LineageOS 23.2 — Android 16, build `BP4A.251205.006`, SDK 36; the fingerprint still reports the stock `cupid_global` Android 15 build (`OS3.0.3.0`). Not MIUI/HyperOS.
- Display: 1080×2400 @ ~422 dpi; modes id 0 = 60 Hz, id 1 = 120 Hz (active at session start). **Both `min_refresh_rate`/`peak_refresh_rate` pinned to 120 for every block** (`--pin-refresh 120`, read-back `peak=120.0 min=120.0`, restored to `Infinity`/`0.0` afterwards); Frust's in-app FPS readout showed 120 in S5. Both budgets (16.67 / 8.33 ms) reported — the 8.33 ms column counts cost sums, not missed vsyncs (see the OnePlus 9 note "What the 8.33 ms column no longer means"); `platform-view period_us` read 8.23–8.24 ms at p50 and p95 on every frame scenario, i.e. the presented cadence is the 120 Hz period.
- High-refresh opt-in engaged on both apps (§6): Frust `Surface.setFrameRate`, Flutter `flutter_displaymode`. Achieved (active frames ÷ 300 s): Frust ~120.5–120.8 fps in every frame scenario; Flutter ~105.3 (S1), ~111.1 (S5), ~119.8–119.9 (S4/S6); S2/S3 Flutter paint is event-driven.
- Brightness fixed 128/255, auto-brightness off (`device_state.sh`); airplane on (read-back 1), Wi-Fi off, Bluetooth off (both on before the session and restored after), battery saver off, screen timeout 30 min, `svc power stayon true` during blocks; all restored and read back at 23:13 UTC.
- Charger: USB/AC-connected throughout (`AC powered: true` at both ends of the session, real level 45 % → 100 %, full from the S7 blocks on); `dumpsys battery unplug` spoofed on-battery state before every block; the `--min-level 30` guard never waited.
- Thermal: gate ceiling 38 °C / 120 s cooldown before every block. Session start 31.9 °C, peak 39.0 °C (after the frust S2 block), end 29.4 °C. **Two cooldown timeouts** (deviation 2); no other block stalled. 30 s blocks 383–385 s wall (506 / 505 s for the two that waited), 60 s S7 blocks 743 s each.
- Toolchain: Rust 1.98.1 (pinned), `wgpu` 30.0.1 (pinned); Flutter 3.47.2 stable / Dart 3.13.2. Frust APK arm64-only, 12,008,167 B (md5 1715cb35…); Flutter APK fat profile build, 70,462,089 B (md5 43132929…) — packaged differently, not a size comparison (see the App size section). Both installed fresh by the driver, md5 re-verified.
- Visual gate (driver screencap per block, all 16 reviewed by bottom-band luma, the frust S5 frame inspected): **content PASS on every block, both apps; no soft keyboard or other foreign window on any capture** (bottom-band luma 19–54 on fifteen blocks; the frust S5 capture's 114 is its own image column). **S5 geometry PASS — Frust's column is edge-to-edge with every visible cell fully decoded.**
- Sanitization: driver staging `RAW_OK` (192 run logs, 384 PSS snapshots, 16 stats.txt, S7 cpuinfo/coldstart), whitelist self-check passed.

### S1

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 7.85 | 10.18 | 11.28 | 63.25 | 40 (0.11%) | 13,916 (38.49%) | 36,155 (~120.5 fps avg) |
| Flutter (profile) | 9.45 | 19.92 | 23.80 | 36.40 | 1,802 (5.70%) | 28,145 (89.06%) | 31,601 (~105.3 fps avg) |

Frust pass p50 (ms): rebuild 0.17, layout 0.00, paint 0.39, encode 0.05, acquire 1.40, submit 5.99; layout_us>0 on 46/36,155 frames; GPU (gpu_q=1 on 36,115) total p50/p95 5.25/5.37 ms

**Frust wins S1** — p50 7.85 vs 9.45 ms, p95 10.18 vs 19.92, p99 11.28 vs 23.80, and 0.11 % vs 5.70 % of frames over the 60 Hz budget, at ~120.5 vs ~105.3 fps; Flutter's worst single frame is lower (36.40 vs 63.25 ms — Frust's worst is the launch frame of each run). The pass breakdown puts 5.99 ms of Frust's 7.85 ms in submit and 5.25 ms of GPU pass time under it, `gpu_main` p50 5.20 ms — S1 is GPU-bound on this device too, 1.2 ms lighter than the Adreno 660's 6.37 ms.

### S2

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 7.09 | 9.05 | 9.68 | 64.85 | 17 (0.05%) | 5,465 (15.09%) | 36,222 (~120.7 fps avg) |
| Flutter (profile) | 14.79 | 25.41 | 28.57 | 41.10 | 1,995 (33.93%) | 5,231 (88.98%) | 5,879 (~19.6 fps avg) |

Frust pass p50 (ms): rebuild 0.07, layout 1.37, paint 0.20, encode 0.05, acquire 0.10, submit 5.54; layout_us>0 on 36,219/36,222 frames; GPU (gpu_q=1 on 36,182) total p50/p95 3.07/3.16 ms

**Frust wins S2 decisively** — p50 7.09 vs 14.79 ms, p95 9.05 vs 25.41, p99 9.68 vs 28.57, and 0.05 % vs 33.93 % of frames over the 60 Hz budget. Frust holds ~120.7 fps continuously while Flutter's paint is event-driven (~19.6 fps of painted frames), so the fps columns are not comparable; the per-frame figures are. Frust re-lays out every frame here (`layout_us>0` on 36,219/36,222) at 1.37 ms median; GPU pass time 3.07 ms.

### S4

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 3.43 | 7.89 | 8.95 | 75.06 | 12 (0.03%) | 1,085 (3.00%) | 36,202 (~120.7 fps avg) |
| Flutter (profile) | 3.92 | 5.64 | 5.92 | 66.49 | 1 (0.00%) | 10 (0.03%) | 35,969 (~119.9 fps avg) |

Frust pass p50 (ms): rebuild 0.18, layout 0.00, paint 0.08, encode 0.02, acquire 0.11, submit 3.01; layout_us>0 on 27/36,202 frames; GPU (gpu_q=1 on 36,162) total p50/p95 0.71/0.77 ms

**Split on S4.** Frust takes the median (3.43 vs 3.92 ms); Flutter takes the tail (p95 5.64 vs 7.89, p99 5.92 vs 8.95) — both at ~120 fps and both essentially never over the 60 Hz budget (0.00 % vs 0.03 %). Frust's GPU pass time is 0.71 ms; the frames that overlap the JSON parse windows set its tail.

### S5

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 6.57 | 7.16 | 7.38 | 66.18 | 10 (0.03%) | 55 (0.15%) | 36,227 (~120.8 fps avg) |
| Flutter (profile) | 5.93 | 10.31 | 12.33 | 26.67 | 26 (0.08%) | 6,561 (19.69%) | 33,326 (~111.1 fps avg) |

Frust pass p50 (ms): rebuild 1.38, layout 0.06, paint 0.10, encode 0.02, acquire 0.11, submit 4.86; layout_us>0 on 36,226/36,227 frames; GPU (gpu_q=1 on 36,187) total p50/p95 5.51/5.63 ms

**Split: Flutter the median, Frust the tail and the cadence.** Flutter p50 5.93 vs 6.57 ms; Frust p95 7.16 vs 10.31, p99 7.38 vs 12.33, 60 Hz miss 0.03 % vs 0.08 %, ~120.8 vs ~111.1 fps. **The capture composites the whole image column**: 35,095 `frust-perf img` lines across the kept runs report `skipped=0` on every one, with 55,422 evictions and up to 63 resident images against the mobile-tier 1024×1024×4 atlas (`transient_saves_memory=true` on the Adreno 730, the same tier as the OnePlus 9) — PROTOCOL §7's content gate is met with eviction-only degradation. Frust's rebuild is 1.38 ms median, which is that re-upload traffic; GPU pass time 5.51 ms.

### S6

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 6.38 | 8.91 | 10.77 | 79.65 | 44 (0.12%) | 3,498 (9.67%) | 36,180 (~120.6 fps avg) |
| Flutter (profile) | 7.82 | 10.05 | 10.90 | 14.45 | 0 (0.00%) | 13,057 (36.34%) | 35,930 (~119.8 fps avg) |

Frust pass p50 (ms): rebuild 0.06, layout 0.08, paint 0.12, encode 0.05, acquire 0.06, submit 5.90; layout_us>0 on 36,180/36,180 frames; GPU (gpu_q=1 on 36,140) total p50/p95 1.19/1.29 ms

**Frust wins S6 on the median and p95; Flutter on the tail's end.** p50 6.38 vs 7.82 ms, p95 8.91 vs 10.05, p99 10.77 vs 10.90; Flutter never misses the 60 Hz budget (0.00 % vs 0.12 %) and its worst frame is 14.45 vs 79.65 ms (Frust's again the launch frame). Frust's GPU pass time is 1.19 ms median — the cost here is shaping and upload on the CPU side, not the strip pipeline.

### S3

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | 12.37 / 18.39 / 25.48 / 51.36 | 352 | 10.10 / 10.17 / 10.17 / 10.17 | 2 |
| create 10k | 12.93 / 23.00 / 25.31 / 26.16 | 360 | **not captured** | 0 |
| update every 10th of 10k | 10.73 / 18.38 / 21.02 / 21.55 | 359 | **not captured** | 0 |
| swap | 10.22 / 16.64 / 19.01 / 20.79 | 354 | **not captured** | 0 |
| clear | 10.34 / 20.08 / 21.15 / 21.35 | 349 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.83 | 8.59 | 15.10 | 64.30 | 174 (0.49%) | 1,810 (5.11%) | 35,444 (~118.1 fps avg) |
| Flutter (profile) | 14.50 | 30.12 | 35.73 | 39.34 | 344 (37.55%) | 854 (93.23%) | 916 (~3.1 fps avg) |

Cycle health — Frust (profile) `s3-create1k` reopens per kept run: 36–36; Flutter (profile) `s3-create1k` reopens per kept run: 19–19.

**Frust wins S3 on every comparable axis.** All five Frust reconcile ops land frames (349–360 each) through the frame-indexed markers, with per-op medians of 10.2–12.9 ms (`create 1k` 12.37) — inside two 120 Hz periods. Flutter lands 2 frames, on `create 1k` alone (its `addTimingsCallback` delivery gap), so the per-op columns are not a like-for-like race. On the overall series Frust is at 4.83 ms p50 / 8.59 p95 over 35,444 painted frames against Flutter's 14.50 / 30.12 over 916 — continuous vs event-driven paint, so read the per-op table for the reconcile cost and the overall table only for cadence.

### S7

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | ~107 ms (108/105/107; first of 4 discarded) | ~379 ms (379/390/368; first of 4 discarded) |
| Framework-reported first-frame span | ~97 ms median (`first_frame_presented`, 84–110 across 12 launches; `adapter_ready` median 35 ms) | ~72 ms median (`first_frame_ms`, 67–90 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | 0 samples | ~0.3% avg (max 0.3%, 2 samples) |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~54.9 MiB (54.6–55.1, 10 kept snapshots) | ~85.2 MiB (84.9–85.4, 10 kept snapshots) |

**Frust starts ~3.5× faster and idles ~36 % lighter.** External cold start 107 vs 379 ms (`am start -W` TotalTime), idle TOTAL PSS 54.9 vs 85.2 MiB. Flutter reports the shorter framework first-frame span (72 vs 97 ms median), measured from each framework's own entry point, so the two spans bound different work; the external `am start` figure is the one measured identically on both sides. Frust did not appear in `dumpsys cpuinfo` during the idle window (0 samples) and Flutter averaged ~0.3 % over 2 samples — idle CPU ~0 % for both.

### S8

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (profile) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 107 | 293 | n/a |
| write i64 | 76 | 286 | n/a |
| write f64 | 75 | 282 | n/a |
| write String | 74 | 255 | n/a |
| write Vec\<String\> | 75 | 249 | n/a |
| read (unique key, forces channel) bool | 22 | 5254† | ~0 |
| read (unique key, forces channel) i64 | 21 | 5254† | ~0 |
| read (unique key, forces channel) f64 | 21 | 5254† | ~0 |
| read (unique key, forces channel) String | 21 | 5254† | ~0 |
| read (unique key, forces channel) Vec\<String\> | 21 | 5254† | ~0 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust wins S8 decisively** — writes 74–107 vs 249–293 µs/call (~2.7–3.8×); a per-key Frust read costs 21–22 µs while Flutter's only channel-crossing read is the whole-store `reload()` at 5,254 µs (its ~0 µs cached read is a Dart-map lookup, not a boundary crossing). Zero `s8-errors` on either app.

### vs 2026-07-21 (vello renderer, release build) — renderer transition, cross-methodology

`harness/compare.py` over two `summarize.py` JSON dumps: the retired vello series, regenerated from the committed `frust_release` raws before their replacement, against this pass; Δ positive = slower.

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new (ms) | Δp95 | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|
| S1 | 4.87→7.85 | +61.1% | 5.77→10.18 | +76.4% | 0.10%→0.11% | 119.7→120.5 |
| S2 | 10.35→7.09 | -31.5% | 16.40→9.05 | -44.8% | 4.23%→0.05% | 65.1→120.7 |
| S4 | 4.50→3.43 | -23.8% | 5.49→7.89 | +43.6% | 0.03%→0.03% | 120.5→120.7 |
| S5 | 5.72→6.57 | +14.7% | 12.29→7.16 | -41.8% | 0.13%→0.03% | 88.6→120.8 |
| S6 | 6.21→6.38 | +2.7% | 11.20→8.91 | -20.5% | 0.19%→0.12% | 88.0→120.6 |
| S3 (overall) | 4.87→4.83 | -0.8% | 9.99→8.59 | -14.1% | 0.26%→0.49% | n/a→n/a |

**Read this as a renderer change under a methodology change, not as a same-renderer delta.** The old pass was a RELEASE build under the pre-§2.5 method, whose `total_us` never included the GPU wait (vello's compute ran asynchronously and was not measured); the engine's `submit_us` waits on the GPU inside the cost sum. So S1's median "+61 %" (4.87→7.85 ms) is bookkeeping — 5.99 ms of the 7.85 is submit with 5.25 ms of GPU pass time under it — the same effect the Renderer transition section records for the iPhone SE. The Flutter control is not fixed either (3.44.2 → 3.47.2; it moved +4.6…+12.1 % on p50 across the same rows). What survives both caveats is the cadence and the tails: **the engine holds 120.5–120.8 fps on every frame scenario where vello dropped to 65.1 (S2), 88.6 (S5) and 88.0 fps (S6)**; p95 falls on S2 (16.40→9.05 ms), S5 (12.29→7.16), S6 (11.20→8.91) and S3 (9.99→8.59); S2's 60 Hz miss rate falls from 4.23 % to 0.05 %. S4's median improves (4.50→3.43) while its p95 rises (5.49→7.89) at the same 120 fps. Off the frame axis: framework first-frame span 87→97 ms median (the profile build now carries the perf trace; the old pass captured no external cold start), idle PSS 52.5→54.9 MiB for Frust against 87.5→85.2 for Flutter; S8 per-op costs are within 10 % of the old pass on both sides.

### Methodology deviations (this device)

1. **Cross-methodology baseline only.** The only earlier pass on this device is the 2026-07-21 vello release-build pass (pre-§2.5 method, Flutter 3.44.2), so the comparison above is a bounded renderer-transition read, not a same-renderer "vs 2026-09-05" delta like the other devices carry.
2. **Two thermal cooldown timeouts.** The flutter S2 block opened at 39.0 °C and the frust S3 block at 38.1 °C; in neither case did the 120 s wait clear the 38 °C ceiling and the driver proceeded (506 / 505 s wall). Both follow the session's peak; no other block stalled and the session ended at 29.4 °C.
3. **APK packaging differs between the two apps** — the Frust APK is arm64-only (12,008,167 B) and the Flutter APK a fat profile build (70,462,089 B); both installed fresh with md5 re-verified; neither figure is a size comparison.
4. **Flutter S2 rows are not comparable to any Flutter S2 row before 2026-09-05** — the Flutter bench app was changed to single-line ellipsis for row-text parity with Frust; the 2026-07-21 Flutter S2 row predates that change.
5. **S3 Flutter per-op capture:** 2 frames on `create 1k` and none for the other ops (the known `addTimingsCallback` delivery gap) — no cross-app per-op comparison.
6. As on prior passes: Flutter release-mode in-app cross-check not captured; S4 parse wall time not recoverable (timestamp-free capture); S8 burst-during-animation variant not run.

---

## App size (release) — Frust Android re-measured 2026-09-17, iOS 2026-09-19, Flutter 2026-09-05

Frust's rows are release builds of this checkout, rebuilt after the app-size
plan's font/link-flag/opt-level/pin levers landed; the Flutter column is
unchanged since the 2026-09-05 `app_size.sh` snapshot of Flutter 3.47.2
(`flutter build apk --release [--split-per-abi]` / `flutter build ios
--release`) and dated accordingly. This pass builds via the `cargo ndk` +
Gradle `assembleRelease -x cargoNdkBuild` route directly (`frust build apk
--release` refuses without release signing and has no
`--no-default-features` knob), so no keystore is configured and Gradle falls
back to debug signing with a warning — signing does not change the size
class either way. Sizes are MiB alongside the exact byte count, which is the
authoritative figure. The iOS rows were **re-measured on a macOS host on
2026-09-19** (Xcode 26.2 / 17C52, `frust build ios --release` — a signed
`Release`/`iphoneos` build with the app's `lean` feature on — the route
`app_size.sh`'s iOS half names), replacing the 2026-09-18 11,716 KB /
11,903,616 B with the rows below; the iOS column has no `db`-off axis
because `frust build ios` has no `--no-default-features` knob either.

**iOS Runner rows.** `frust build ios --release` (a plain `xcodebuild build`) ships this
project's Release Runner **unstripped** — `COPY_PHASE_STRIP = NO`, no
`DEPLOYMENT_POSTPROCESSING` override, so `[profile.release]`'s `strip = "symbols"` never
reaches it (Xcode links the Rust staticlib in, untouched by cargo's own strip). The
stripped row below is a scratch-copy full `xcrun strip` (`app_size.sh`; the artifact
itself is never modified) — measured within 0.29 % of a real `xcodebuild archive` Runner
with the same symbol count (dated note below), so **it, not the unstripped row, is the
figure comparable to Android's stripped-`.so` rows above.**

| Axis | Frust, `db` on (default) | Frust, `db` off (`--no-default-features --features lean`) | Flutter (2026-09-05) |
|---|---|---|---|
| Android universal release APK (3 ABIs) | **24.58 MiB** (25,775,054 B) | not built as universal | 49.36 MiB (51,762,140 B) |
| Android arm64-v8a split APK | **8.57 MiB** (8,981,843 B) | **6.63 MiB** (6,953,291 B) | 17.45 MiB (18,295,458 B) |
| in-APK `lib/arm64-v8a/libfrustbench.so` | 8.43 MiB (8,837,768 B) | 6.49 MiB (6,809,216 B) | 16.55 MiB (engine + app) |
| on-disk arm64 `.so` (stripped), 2026-09-19 | 8.50 MiB (8,912,136 B) | 6.55 MiB (6,865,680 B) | n/a |
| iOS release `.app` (`du -sk`), 2026-09-19 | 11.30 MiB (11,568 KB) | not built | 16.64 MiB |
| iOS `Runner` binary (unstripped), 2026-09-19 | 11.21 MiB (11,755,184 B) | not built | n/a |
| iOS `Runner` binary (**stripped**, plain `xcrun strip` scratch copy), 2026-09-19 | **8.16 MiB** (8,559,536 B; real `xcodebuild archive` Runner measures 8,534,544 B, −0.29 %) | not built | n/a |

The on-disk `.so` row was re-measured on 2026-09-19, after `jni` and then `tokio` and the three
accesskit crates left the cold set (per-lever table below); the other Android rows were not, and
predate both changes. Against it, the arm64 split-APK and in-APK `.so` rows read low by at least
those two per-lever deltas (`jni` +30,184 B lean / +30,576 B db-on; `tokio` + accesskit
+21,944 B lean / +39,384 B db-on, each measured on its own base). The universal APK carries one
`.so` per ABI, so it likely reads low by more than that — not measured. The in-APK and on-disk
`.so` rows are also not directly comparable: beyond those two changes, their lean gap includes
~4.3 KB of unrelated work that landed between the measurements.

**Per-lever contributions**, each measured on the lean arm64 release `.so` of
its own base by the card that landed it — quoted as measured, never summed
into a total; Frust's totals above come only from re-measurement (the 2026-09-17 pass, or
2026-09-18 where a row says so):

| Lever | Base → after (B) | Δ (B) | Note |
|---|---|---|---|
| Material Roboto Flex wght-only instance | 9,471,648 → 7,962,896 | −1,508,752 | font file 1,684,624 → 175,900 B. Render parity is **not device-verified** on either platform: the 2026-09-18 device legs below used material3-demo's landing screen, which shapes in the platform's system UI face and never requests Roboto Flex, so neither leg could test the font (a wght-only instance at default axis values rendering identically remains plausible from the instancing itself; it is unverified on a device). **iOS-relevant**: the face is linked into the Mach-O on iOS too (found verbatim in `__TEXT` in both trees' `frust_bench` binaries); it accounts for 1,508,724 B of that section's `__TEXT.__const` drop of 1,515,120 B (2,478,804 → 963,684) across the same two trees |
| shadcn Inter instance (font bytes) | 879,708 → 636,684 | −243,024 | `frust-shadcn` is not in this bench app, so it does not appear in these artifacts |
| Glyph italics | measured-keep | 0 | no italic subset landed |
| `.cargo/config.toml` `--pack-dyn-relocs=android` + `--icf=all` | 9,471,776 → 9,033,120 | −438,656 | `.rela.dyn` 310,680 → 41,384 B; `ANDROID_RELA` confirmed on the shipped `.so` and the artifact run on-device 2026-09-18 (see below). **Android-only**: `.cargo/config.toml` scopes both flags to the four `*-linux-android*` targets, so no iOS artifact is affected |
| Per-crate `opt-level = "z"` cold set (17 crates, `wgpu-hal` excluded) | 7,523,288 → 6,833,696 | −689,592 | `.text` −17.9 %; unwind tables measured-keep (−645,376 B rejected — removes native backtraces); **the 5 % render-CPU bar was adjudicated on the OnePlus 9 on 2026-09-17 and cleared** — see the dated note below. **iOS-relevant**: `[profile.release.package.*]` is not target-scoped, so the cold set applies to `aarch64-apple-ios` as well — with a second-order cost there, see the iOS-delta note below. `jni`, `tokio` and the three accesskit crates have since left this set (rows below) |
| `jni` removed from the cold set, 2026-09-18 | 6,812,880 → 6,843,064 | +30,184 | db-on +30,576 B (8,841,504 → 8,872,080). The render-CPU bar could not see `jni`'s per-frame work: Android's per-frame JNI polls run in separate JNI entries after `nativeOnFrame` returns, outside every span `total_us` sums. Removed rather than kept on an unmeasured exception. The bar does see per-frame cost inside the frame's own spans — the submit-path cost S5 shows below comes from the cold set |
| `tokio` and the three accesskit crates removed from the cold set, 2026-09-19 | 6,843,736 → 6,865,680 | +21,944 | db-on +39,384 B (8,872,752 → 8,912,136). The render-CPU bar could not see either one's per-frame work (the reactive pump; the semantics publish on every frame whose semantics changed), so both were measured directly instead — see the microbench note below: at `z`, accesskit's publish failed the cold set's CPU rule on a little core (at a 2,000-node tree), and `z` saved `tokio` no size. Removed on the owner's decision after that pass |
| `parley` 0.11.1 pin (single `skrifa`/`read-fonts` copies) | 6,833,696 → 6,812,656 | −21,040 | **iOS-relevant** (a workspace dependency, not target-scoped); its iOS share is folded into the `__TEXT.__text` figure in the note below and was not isolated |
| `jni` 0.21 pin | wont_fix | 0 | `android-activity` requires `jni` `^0.22.4` |
| `android_logger` regex feature off | dependency hygiene | ~0 | |
| bundled-fonts opt-out feature | default on | 0 in these artifacts | an app opting out saves its own bundled font bytes. Measured once on iOS, on `material3-demo` (not this bench app): `Runner` 18,383,328 → 18,019,504 B, −363,824 with `default-features = false` on its `frust-material` row — the two faces (175,900 + 181,388 B) plus their table padding, and neither sfnt is left in the binary |

**Cold-set 5 % CPU bar — adjudicated 2026-09-17 (OnePlus 9), cleared.** The bar the
`opt-level = "z"` row above is gated on was re-measured under control and passes on every
scenario. Two `--profile` APKs differing only in the cold set (arm64 `.so` 9,134,208 B with it,
9,878,576 B without) were interleaved B,V,B,V per scenario — 12 runs × 30 s per block, each
block's own first 2 runs discarded — on a OnePlus 9 over USB adb with airplane mode on, refresh
pinned to 120 Hz and brightness fixed. That order spreads session drift across both variants
rather than cancelling it (an ABBA order would cancel it outright), and the figures below are
the **uncorrected** ones. That is still the conservative choice: the measured B1→B2 drift is
positive in every scenario, so correcting for it shrinks exactly the deltas that could threaten
the bar (S5 +3.4 → +2.9 %, S4 +1.0 → +0.9 %). It moves the two negative deltas the other way —
S1 −1.0 → −2.0 %, S6 −0.3 → −0.3 % — which costs the cold set nothing, since a negative delta is
the cold set running faster. Cost-sum p50/p95 (cold set vs none, mean of each variant's two blocks):
S1 −1.0 %/+0.5 %, S4 +1.0 %/−0.4 %, S5 +3.4 %/+3.3 %, S6 −0.3 %/+0.0 %. CPU-work phases
(rebuild+layout+paint+encode) p50: −0.3 %, −3.0 %, −0.1 %, +0.8 %. S1 `gpu_main` 6.31 ms both
variants (+0.01 %); all sixteen blocks held ~120 fps. S5 is the one real signal — both cold-set
blocks put ~0.2 ms more in `submit` (5.58/5.59 → 5.78/5.80 ms) on the image-upload path; no
per-crate isolation was run, so the responsible member is not established (`wgpu-core` is the
plausible candidate). It passes, but the headroom is not wide: S5's +3.4 % leaves 1.6
percentage points against an S1 block-order drift of 2.15 % on the same rig. Unlike the Pixel 5
attempt, this rig's own drift is smaller than the bar: re-running the *identical* no-cold-set
binary later in the session moved cost-sum p50 by 0.04–2.15 % per scenario, against that
session's +14 %/+24 %. What the bar covers is the frame's own spans. It never measured
`jni`'s per-frame work, which falls outside them: Android's per-frame JNI polls run in separate
JNI entries after `nativeOnFrame` returns, outside every span `total_us` sums, however often they
execute. `jni` was therefore removed from the cold set on 2026-09-18 (per-lever table above)
rather than kept on an unmeasured exception, which also retires the focused-text-field S1 leg
this pass could not run. `jni` was not the only cold-set crate with per-frame work outside those
spans: `tokio`'s runtime-context entry (`Handle::enter`)
runs at every reactive pump — at least four times a frame: once before the rebuild span, then at
the top of `native_ime_state` and of both clipboard drains (`native_take_clipboard_write`,
`native_take_paste_request`), with more on frames that carry input — and `frame()` calls the accesskit semantics publish between the layout
and paint spans. This bar could not measure either; both were measured directly instead (next
note) and have since left the cold set. A further caveat: `dumpsys battery unplug` only made
the framework report unpowered — the phone stayed on USB power with its level pinned at 100 %,
so PROTOCOL §3's charger-off control was not literally met and these absolute numbers are not
comparable to battery-run passes.

**`tokio`/accesskit per-frame cost, settled by microbench — OnePlus 9, 2026-09-18.** The bar
above sums only the frame's own spans and cannot resolve either crate's per-frame work (previous
paragraph): on this rig the bar's own block-order drift is 0.04–2.15 % of S1's 8.55 ms cost-sum
p50 (≈3–180 µs), while a `tokio` runtime-context entry's or an accesskit publish's `z`-vs-`3`
*difference* — the quantity this pass measures, not either operation's own total cost (a single
N=2000 publish costs 1.4–5.9 ms on its own; see the results below) — is expected to cost well
under that. A frame-level A/B would "pass" with no power to detect a difference that small. This
pass uses a sub-microsecond instrument instead, against the **pre-registered rule fixed before
measuring**: a crate stays in the cold set iff its measured per-frame extra cost at `z` vs `3` is
< 0.5 % of S1's cost-sum p50 (0.5 % of 8.55 ms = **42.75 µs** — this is the threshold itself, not
a base to take a further 0.5 % of), on the worse of a big core and a little core.

*Method.* Two standalone `aarch64-linux-android` bench crates, outside this repo's workspace
(own `[workspace]`, `[profile.release]` hand-copied from this checkout's
`lto`/`codegen-units`/`strip`/`panic`, `Cargo.lock` copied from this checkout so every shared
dependency resolves identically — confirmed on build: `tokio` 1.53.1, `any_spawner` 0.3.0,
`reactive_graph` 0.2.14, `accesskit` 0.24.1), each built twice, byte-identical except the one
crate's own `opt-level` override under test (`z` vs `3`; the four resulting binaries were
confirmed to differ pairwise by `md5`). Sources and raw per-block logs are attached to the task
that produced this pass. The `tokio` bench path-depends on `frust-reactive` and calls
`ReactiveRuntime::pump_local()` directly — idle queue, and with one task (spawned via
`any_spawner::Executor::spawn_local`) parked on `tokio::time::sleep(1h)`, registered once outside
the timed region so the pump has a live registered timer in scope without being re-polled each
call. The accesskit
bench depends on `accesskit` alone — matching `frust-core`'s own dependency; `accesskit_consumer`/
`accesskit_android` never entered either build's graph, so only `accesskit`'s own override was
exercised (the card allows this: "only accesskit matters for the TalkBack-off path") — and times
one "publish": build N `accesskit::Node`s the way `frust-widgets/src/button.rs`'s `semantics()`
does (`Role::Button`, bounds from a synthetic per-row layout, a text label, `Action::Click` or the
disabled state — a representative mix, every 5th node disabled), clone the `Vec` (mirroring
`tree_update_from_semantics`'s `update.nodes.clone()`), clone again (mirroring
`publish_semantics`'s `tree_update.clone()` into the late-activation snapshot slot), drop all
three — the TalkBack-off cost `frust-shell-android`'s frame path pays on every changed-generation
frame regardless of whether a screen reader is listening. Timing is `std::time::Instant`-batched
(several batches of many iterations each; the median batch ns/iter is reported, with min/max as
the spread). Cores are `taskset`-pinned: little = cpu0 (A55). Big was meant to be one fixed index
in cpu4-6 (A78), but this rig's Qualcomm `core_ctl` dynamically isolates individual big cores
under low load (`/sys/devices/system/cpu/cpuN/isolate` flips a single core's `sched_setaffinity`
to `EINVAL` for anywhere from seconds to ~2 minutes, confirmed via the `isolate` sysfs file, not a
thermal effect — `dumpsys thermalservice` read Thermal Status 0/NONE throughout), so the big-core
mask actually used is the cpu4-6 **union**, letting the scheduler place each run on whichever of
the three is currently de-isolated — still "a big core" per the card, not a fixed one. ABBA order
`z,3,3,z` × 2 rounds per (workload, core) combination. Each of the four runs in a round collapses
to its own median ns/iter (the bench binary's own batch median); a round's drift-cancelled delta
is `mean(the two z-position runs' medians) − mean(the two 3-position runs' medians)`, and the
reported delta is the mean of the two rounds' deltas — the same arithmetic the ABBA driver script
computes. The result tables' `z`/`3` columns below report that same mean-of-medians, not a
straight median of the four runs — the two differ slightly (e.g. accesskit's little core: 91,847
ns from the mean-of-medians below vs 89,708 ns from a plain median-of-the-four instead), so read
the columns as "mean of the two same-variant runs' own medians," matching how Δ is built. Battery
temperature was sampled at the start and end of each script invocation (bracketing its 2 rounds,
not sampled between individual rounds) and stayed flat within every such bracket (`dumpsys
battery`, at most 0.8 °C drift start-to-end, always on USB power — PROTOCOL §3's charger-off
control is not met here either, the same caveat as the rest of this device's rows).

*Positive control.* A ~1 µs `Instant`-based busy-wait (accurate independent of CPU frequency)
injected into the same timed loop, baseline vs injected: `tokio` bench (idle workload, big core)
51.2 ns/iter → 1179.6 ns/iter, **delta 1128 ns**; accesskit bench (N=1 — at N=500 the loop's own
~2 µs batch-to-batch band swamps a 1 µs signal against a 344 µs baseline, so the control was
re-run at N=1 to get a clean baseline) 752.7 ns/iter → 1881.0 ns/iter, **delta 1128 ns**. Both
resolve the ~1 µs injected signal to within ~13 %, validating that the batched-`Instant` timing
methodology behind every result below correctly recovers a known signal at that scale — the real
deltas measured below are not themselves ~1 µs (`tokio`'s are two orders of magnitude smaller;
accesskit's at N=2000 are up to three orders of magnitude larger), so this control speaks to the
instrument's fidelity, not to a shared magnitude with either bench's own result.

*A/A noise floor* (same binary run twice back to back, no ABBA): `tokio` idle, little core
**1.07 ns**/pump, big core **1.45 ns**/pump — both far below the real deltas the little core shows
below. accesskit N=2000, little core **16.8 µs**/publish (0.29 % of its own 5.88 ms baseline), big
core **7.2 µs**/publish (0.51 % of its own 1.43 ms baseline) — the big-core accesskit delta below
sits *inside* this noise floor and is flagged low-confidence on its own; the little-core delta is
~5.5× this floor and clearly resolved.

**`tokio` result** (ns/pump, ABBA-cancelled `z − 3`; `z`/`3` columns are each the mean of that
variant's two same-position runs' own medians, per the Method note above):

| Workload | Core | `z` | `3` | Δ (`z − 3`) |
|---|---|---|---|---|
| idle (empty queue) | little (cpu0) | 232.4 ns | 220.3 ns | **+12.11 ns** |
| idle (empty queue) | big (cpu4-6) | 52.10 ns | 52.09 ns | +0.01 ns (noise) |
| timer (one parked task) | little (cpu0) | 259.4 ns | 244.6 ns | **+14.75 ns** |
| timer (one parked task) | big (cpu4-6) | 54.35 ns | 52.95 ns | +1.40 ns (≈ its own A/A floor) |

Worst case is the little core's timer workload. Per-frame extrapolation (≥4 pumps/frame, per the
card): 14.75 ns × 4 = **59.0 ns/frame**, which is **0.14 % of the 42.75 µs threshold** — far under
the threshold. **`tokio` meets the rule.**

**accesskit result** (ns/publish, `z − 3`; `z`/`3` columns are each the mean of that variant's
two same-position runs' own medians, per the Method note above):

| N | Core | `z` | `3` | Δ (`z − 3`) | Method |
|---|---|---|---|---|---|
| 2000 | little (cpu0) | 5.870 ms | 5.778 ms | **+91,847 ns** | full ABBA, 2 rounds |
| 2000 | big (cpu4-6) | 1.414 ms | 1.413 ms | +1,075 ns | full ABBA, 2 rounds — inside the big-core A/A floor, low confidence alone |
| 500 | big (cpu4-6) | 344.4 µs | 338.9 µs | +5,494 ns | single order (`z` then `3`), informative only, no ABBA |
| 100 | big (cpu4-6) | 58.72 µs | 60.29 µs | −1,572 ns | single order, informative only; opposite sign — at very small N the direction is not established |

The N=500/N=100 rows are supplementary (single-order, not ABBA-controlled, and only run on the
big core — see *Largest tree* below for why N=2000 is the number the rule actually uses). At
N=2000, the little core reads **91,847 ns/publish = 2.15× the 42.75 µs threshold** — clearly
resolved, at ~5.5× the little core's own A/A floor. The big core's N=2000 point, taken alone, is
**1,075 ns = 2.5 % of the threshold** and would *pass* on that core by itself (and sits inside
its own 7.2 µs A/A floor, so it is not a confidently-resolved signal either way) — but the rule
is defined on the worse of the two cores, and the little core is unambiguously worse here, so
**accesskit fails the rule at N=2000**. This was measured only at N=2000 on the little core (the
value the rule uses) plus the two single-order, big-core-only points above; whether the little-core
delta scales with N was not measured. If it scaled linearly (untested), the 91,847 ns delta at
N=2000 implies roughly 46 ns/node, which would cross the 42.75 µs threshold near **N ≈ 930** — a
hypothetical extrapolation from one measured point, not a second measurement, and not the basis
for the verdict above.

**Largest `frust_bench` scenario tree — not established by a host run; N=2000 used as the
conservative stand-in per the card's own fallback.** Building a headless host harness that
constructs a scenario's `View`/`Widget` tree and calls `RenderRoot::semantics().nodes.len()` was
out of scope for this pass's time budget, so no scenario's real node count was measured. What
follows is code-inspection **inference**, not a measurement: S2 (`s2_list.rs`) materializes only
the visible viewport plus `BUFFER_ROWS = 3` above/below (`ITEM_EXTENT = 72.0`, so a ~2400 px-tall
device viewport would hold ~33 rows, +3+3 buffer ≈ 39 materialized rows by this reading of the
constants alone — `CACHE_MARGIN_ROWS = 12` extends the *shaped-text* cache, not the semantics
tree); S3 (`s3_table.rs`, `ROW_EXTENT = 40.0`) reads as a similarly virtualized grid by the same
kind of inspection. Neither scenario's live tree was counted, so "far below 2000 nodes" is an
inference from the constants, not a measured bound; it is offered only to argue that N=2000 is
plausibly a conservative (harder-to-pass, not easier-to-pass) stand-in, per the card's own
fallback intent — not as evidence about the real crossing point.

**Size give-back (`tokio`).** Measured the same way as the `jni` row above: the lean arm64
release `libfrustbench.so` (`benchmarks/frust_bench`, `cargo ndk -t arm64-v8a build --release
--no-default-features --features lean`) and the `db`-on default build, each built twice —
`[profile.release.package.tokio]` present (`z`, the current committed state) vs a temporary local
removal of that block from `benchmarks/frust_bench/Cargo.toml` (falls back to the profile's `3`
default), reverted after each measurement and confirmed by `git status`/`git diff` showing no
change to that file. `cargo ndk` reused every unaffected cached object both times (only `tokio`
and its dependents recompiled), so the two `.so`s in each pair differ only in that one override.

| Build | with override (`z`, present) | without (`3`, removed) | Δ (removed − present) |
|---|---|---|---|
| lean (`--no-default-features --features lean`) | 6,844,248 B | 6,843,000 B | **−1,248 B** |
| `db` on (default) | 8,873,264 B | 8,873,568 B | **+304 B** |

Both "with override" (`z`, present) builds above came out **1,184 B above** the committed
reference rows (6,843,064 B lean / 8,872,080 B db-on) — the expected gap from toolchain/date
drift since that pass; the "without" (`3`, removed) builds have no reference row to compare
against. Unlike `jni`'s row (a consistent +30 KB in both builds when removed), `tokio`'s sign
**flips between the two builds** — a section breakdown of the lean pair (`llvm-size -A`, on the
exact two `.so` files the 6,844,248 B / 6,843,000 B row above was measured from) shows why: under
`z`, `tokio`'s own `.text` shrinks (−12,312 B vs `3`), but `.eh_frame`/`.eh_frame_hdr` grow by
almost exactly as much (+8,292 B / +3,928 B = +12,220 B combined — more, smaller functions under
`z` means less inlining and more per-function unwind-table entries), and
`.rodata`/`.data.rel.ro`/`.rela.dyn` add a further +808 / +544 / +51 B; summed over every section
`llvm-size -A` lists (including `.got` −16 B and the `.relro_padding` alignment gap −1,248 B),
these sections net **+47 B** larger under `z` — which is not the same figure as the lean row's
**+1,248 B** `wc -c` delta above (the authoritative one for the give-back table): 1,201 B of that
gap falls outside what `-A` lists per-section (ELF section-header-table/string-table bytes and
inter-segment alignment, not attributed to a named section here) and is not decomposed further.
Both figures agree on direction and rough scale: **`opt-level = "z"` buys `tokio` no reliable size
reduction in this binary** — the size axis gives no reason to keep it in the cold set even though
the CPU rule above says it may stay.

*accesskit's give-back, measured the same way (optional per the card, all three of its cold-set
overrides removed together, lean build only):* 6,844,248 B (present) → 6,867,728 B (removed),
**Δ +23,480 B** — a real, one-directional cost, the same shape as `jni`'s row (unlike `tokio`'s
above). Removing accesskit from the cold set, which its CPU result above recommends, is not free
on size; that tension is a conductor-level call, not one this pre-registered CPU-only rule
resolves.

**Outcome under the pre-registered rule.** `tokio`: **meets the rule** — its worst-case per-frame
extra (little core, timer workload) is 0.14 % of the 42.75 µs threshold, far under the
threshold — stays. accesskit: **fails the rule at N=2000**, the value the rule uses — its little-core
delta (the worse of the two cores, and the one clearly resolved above its own A/A floor) is 2.15×
the threshold; its big-core delta alone would meet the rule (2.5 % of the threshold) but sits
inside its own A/A floor and is not the core the rule selects. Recommended out, on the same
unmeasured-per-frame-work grounds `jni` already left on, though unlike `jni`'s size story,
removing accesskit would cost size rather than give it back. Applied on 2026-09-19:
on the owner's decision, `tokio` and all three accesskit crates left the cold set (per-lever
table above) — accesskit on this rule's outcome, `tokio` because `z` saved it no size.

**Device legs — OnePlus 9, 2026-09-18 (release builds).** The two owed on-device checks for
the font and link-flag levers above.

*Material font parity — measured, but this screen could not test the font.* material3-demo's
landing screen, built from the merged tree, matches the same screen built from `dev` @
`f8fcd07b` (the full pre-instance Roboto Flex) at mean |ΔLuma| **0.000002** outside the status
bar — max 1, five differing pixels out of 2,462,400 — against a two-capture noise floor of
exactly **0.000000** per build. That is a true measurement of the screen, but not of the font:
the landing screen's text never asks for Roboto Flex. The app bar title, the navigation-bar
labels and the list-row text are plain `text(..)` views with no `.family(..)`, so they take
`TextStyle::default()`'s `FontFamily::SystemUi`, and `TextWidget::effective_style` copies only
the colour from the theme, never the family — the whole screen shapes in the platform's system UI
face, whatever `frust-material` bundles. (Components that read `theme.type_scale` — buttons,
tabs, text-field labels — do get Roboto Flex.) So this leg has **no power** over the font lever,
and was first recorded here as a pass in error. Nothing measured shows the instance rendering
*differently* either; a wght-only instance at default axis values rendering identically remains
plausible from the instancing itself, but a real device check needs a screen built from
type-scale components, and none was run on either platform (see the iPhone SE leg below, which
proved the same blind spot with negative controls). `frust_bench` S6 cannot be adjudicated this
way and is reported as such rather than scored: it animates a width-driven relayout, so two
captures of the *same* build already differ by mean 21.42 (merged) / 21.55 (baseline), and the
cross-build figure, 13.91, is **below either build's own floor** — no font-attributable
difference is detectable above animation phase. A still-frame luma bar is simply the wrong
instrument for a live scenario. (The owed leg named a material3-demo "typography screen"; the
app has no such screen — its sections are Do/Pick/View/Nav/Find — so its landing screen was
used instead; as above, that screen does not render the Material type scale's family.)

*Packed relocations + ICF.* `llvm-readelf -d` on the shipped lean arm64 release `.so`
(6,813,384 B, in a 6,957,455 B APK) reports `ANDROID_RELA` at `0x60000011` with
`ANDROID_RELASZ` 42,104 B, and `.rela.dyn` carries section type `ANDROID_RELA` — the packed
format, against the 310,680 B unpacked figure in the table above — with `BIND_NOW` set. The
APK installs, launches and sustains a full S1 block (12 × 30 s) with **zero crashes and zero
process-liveness failures**, mean TOTAL PSS 113.6 MiB (`dumpsys meminfo` reports KiB; the
12-run mean is 116,328.5 KiB), and a launch screencap carrying 238 distinct luma levels with
97.01 % of pixels above 8/255 outside the status bar — real content, so `--icf=all` has not
produced the blank-surface failure its accepted risk class would imply. **No frame percentiles
come from this leg**: a release build compiles in no `perf-trace`, so `frust-perf raw` emits
nothing (verified: 0 lines), and SurfaceFlinger's `--latency` ring returns no frame rows for
the app's BLAST layer on Android 15, so present cadence could not be sampled either. This leg
evidences load-and-run correctness, not performance.

**Device legs — iPhone SE (2nd gen), 2026-09-18.** iOS 26.7 (23H24), Xcode 26.2 / 17C52,
`devicectl` 506.6, Developer Mode on. **Transport was wireless throughout — no cable** (the
handset was `unavailable` to CoreDevice for the first few minutes of the session and
re-appeared on its own once a tunnel was acquired), and every capture's status bar carries the
charging glyph, so the device stayed on power: PROTOCOL §3's charger-off control is not met
here either. Brightness, thermal and radio state remain uncontrolled — no iOS CLI exposes them.

*A screenshot path on a physical iPhone now exists.* `devicectl` still has no screenshot
subcommand and libimobiledevice's `screenshotr` is refused on iOS 17+ ("Invalid service"), so
this leg drove `XCUIScreen.main.screenshot()` from a host-app-less XCUITest bundle built
outside the repository, signed under the existing team's development profile, and pulled the
PNGs out of the result bundle with `xcrun xcresulttool export attachments` — 750 × 1334, 8-bit
RGB, one per capture. Comparisons crop the top 40 px (the 20 pt status bar) and score BT.709
integer luma over the remaining 970,500 px. This supersedes the "no iOS screenshot CLI" clause
in the 2026-09-06 pass's deviations for future passes; that pass had no such gate.

*Capture protocol: uninstall before every install.* An in-place `devicectl device install app`
over an existing install leaves the previous build's glyph rasterisation in effect. Two capture
sets taken that way differ from clean-install captures by mean |ΔLuma| **0.48**, confined to
the text rows, with ink-run boundaries agreeing to ±1 px — the same face rasterised differently,
and enough on its own to swamp a < 1/255 bar. Only clean-install captures are reported below.

*Material font parity — bar met, but the screen has no power to test it.* material3-demo's
landing screen (the app has no "typography screen"; its sections are Do/Pick/View/Nav/Find —
the same substitution the OnePlus 9 leg made) is **pixel-identical** between the merged tree
and `dev` @ `f8fcd07b`: mean |ΔLuma| **0.000000**, max 0, 0 differing pixels of 970,500. Both
floors are 0.000000 too — two captures of one running process, and two independent clean
installs of the same build (byte-identical PNGs). But that same screen is *also* pixel-identical
when `frust-material` is built `default-features = false` (no font bytes linked at all) and when
its bundled Roboto Flex is byte-replaced by Space Mono (found byte-for-byte in that build's
`Runner`, with Roboto Flex absent). The reason is the one given for the OnePlus 9 leg above:
Roboto Flex *is* registered — `frust_material::install()` runs before the shell's
construction-time font drain, and a per-frame drain follows — but this screen never asks for it.
Its text is plain `text(..)` with the default `FontFamily::SystemUi`, and the themed text path
copies only the colour, so every build shapes in the system UI face (San Francisco on iOS)
through frust's own parley/fontique stack. The < 1/255 result is therefore recorded as **an
instrument with no power**, not as evidence that the instanced face renders identically to the
full one. (Supporting observation only: the handset carries no Roboto family — 86 CoreText
families, and CoreText resolves `"Roboto Flex"` to Helvetica — but frust does not shape through
CoreText, so that lookup is not the mechanism.) A device check of the font lever needs a screen
built from type-scale components (buttons, tabs, text-field labels); none was run.

*bundled-fonts opt-out on iOS.* The `default-features = false` build (temporary manifest patch,
reverted; `Runner` 18,383,328 → 18,019,504 B, with neither sfnt left in the binary) launches and
renders the whole screen with **no missing glyphs and no `.notdef` boxes**, in the system UI
face. That face is not a fallback here: the bundled, fonts-off and Space-Mono builds all render
this screen in San Francisco, which is exactly why the screen cannot test the bundled font. What
the leg does show is that turning `bundled-fonts` off leaves nothing on this screen unrendered.

*S6 (text-heavy) after the `harfrust` 0.10 → 0.12 / `parley` 0.11.0 → 0.11.1 move.* One full
PROTOCOL §4 block from the merged tree (`--profile`, `FRUST_TRACE_RAW=1`, 12 × 30 s, first 2
discarded, 18,201 frames) against this device's recorded 2026-09-06 S6 (18,505 frames). Its raw
logs were kept outside the repository and are **not** committed under `raw/iphone_se/`. On the
per-frame comparison row — `total − acquire` — the median is lower: p50 5.57 → 5.45 ms
(−2.2 %), with the two per-run p50 bands not overlapping (recorded 5.55–5.60, new 5.43–5.47).
p95 lands inside the recorded spread (6.28 → 6.32 ms; bands 6.21–6.32 vs 6.25–6.36), the 60 Hz
miss rate on the work row is 0.00 % in both, GPU pass time is unchanged (p50/p95 1.64/1.86 →
1.65/1.86 ms), and the shaping-side pass costs are flat — layout p50 0.16 → 0.16 ms, paint
0.25 → 0.24 ms — while rebuild p50 rose 0.07 → 0.10 ms. **The tail is worse and unexplained**:
p99 6.64 → 7.07 ms (+6.5 %), worst 13.80 → 14.52 ms, and frames over 8.33 ms 12 → 30. The block
also crosses an OS change (26.6.1 → 26.7), a different transport and twelve days of unrelated
tree movement, and those confounds could hide a regression as easily as they could produce the
lower median. What this block supports is **no median regression and flat layout/paint p50**
after the shaper move — not a gain, and not a clean tail.

**iOS delta, and where it went — 2026-09-18.** The move from the 2026-09-06 iOS rows
(11,908 KB / 12,103,152 B) is not attributable to the app-size levers: twelve days of
unrelated work sit between those rows and these. So the same signed `Release`/`iphoneos`
build was made twice on one host minutes apart, same Xcode and same pinned rustc, once from
`dev` @ `f8fcd07b` — the last `dev` commit before the size-lever commits, and the same baseline
the font-parity legs use — and once from the merged tree (that window also carries a few
unrelated commits): `.app` 12,008 → 11,716 KB, `Runner`
**12,203,888 → 11,903,616 B (−300,272 B, −2.46 %)**. That net is far smaller than the levers
themselves, and the Mach-O says why (`otool -l` + `size -m`):

| Segment / section | `f8fcd07b` | merged | Δ (B) |
|---|---|---|---|
| `__TEXT` | 9,469,952 | 7,094,272 | **−2,375,680** |
| — `__TEXT.__const` | 2,478,804 | 963,684 | −1,515,120 |
| — `__TEXT.__text` | 6,626,664 | 5,775,428 | −851,236 |
| `__DATA_CONST` | 294,912 | 311,296 | +16,384 |
| `__LINKEDIT` | 2,389,872 | 4,448,896 | **+2,059,024** |
| file total | 12,203,888 | 11,903,616 | −300,272 |

The levers did land — `__const` falls by almost exactly the font instance (1,684,624 →
175,900 B, both blobs located verbatim in their binaries) and `__text` by 851,236 B — but
**87 % of that is given back by `__LINKEDIT`**, which here is symbol material: the symbol
string table 1,589,592 → 2,821,216 B (+1,231,624), the nlist table 20,696 → 33,778 symbols
(+209,312 B) and the exports trie 317,576 → 927,960 B (+610,384). **The iOS `Runner` from a
plain `xcodebuild build`/`frust build ios --release` is not stripped.** Xcode links the Rust
`staticlib` into it and `xcodebuild build` runs no strip phase, so `[profile.release]`'s
`strip = "symbols"` governs cargo's own links and never reaches this artifact. Stripping
copies of both binaries (`xcrun strip -x -S`, scratch copies — the measured artifacts were
not modified) gives 10,894,880 → 10,235,120 B, **−659,760 B (−6.06 %)** — measured with
`-x -S`, which keeps every global symbol, so it undercounts what an archive actually strips
(dated note below). The symbol growth tracks the `opt-level = "z"` cold set: the largest
per-crate symbol-count
increases are `wgpu_core` +554, `naga` +354, `tokio` +241, `wgpu_types` +120, `image` +60 and
`png` +47 — members of the cold set at the time (`tokio` has since left it) whose functions
survive as distinct symbols under `z`. That is *consistent with* the cold set, not proven by
it; no per-lever iOS isolation build was made.

**`xcodebuild archive` measured directly, settling the open archive question — 2026-09-19.**
`(cd benchmarks/frust_bench/ios && xcodebuild -project Runner.xcodeproj -scheme Runner
-configuration Release -sdk iphoneos -destination 'generic/platform=iOS' -archivePath
<path> archive)`, with the same `FRUST_FEATURES=bGVhbg==` (`lean`, base64)/`DEVELOPMENT_TEAM`/
codesigning environment `frust build ios --release` passes, succeeds (`** ARCHIVE
SUCCEEDED **`) and produces an archived Runner of **8,534,544 B**, against the same
commit's plain `xcodebuild build` Runner (**11,755,184 B** — this pass's App-size table
unstripped row above): **−3,220,640 B (−27.40 %)**. `size -m` shows `__TEXT` (7,143,424 B),
`__DATA_CONST` (311,296 B) and `__DATA` (65,536 B) byte-identical between the plain build and
the archive — the compiled code does not change — and only `__LINKEDIT` differs: 4,259,840 →
1,032,192 B. `otool -l`'s `LC_SYMTAB` explains why: `nsyms` 32,451 → 522 (`nm` count
27,379 → 521); the `LC_DYLD_EXPORTS_TRIE` is untouched at 877,000 B both times.

A **plain (no-flags) `xcrun strip`** on a scratch copy of the same plain-build Runner
matches the archive almost exactly: **8,559,536 B**, `nm` **521**, `__LINKEDIT`
**1,064,960 B** — within **24,992 B (0.29 %)** of the archive's 8,534,544 B, with the same
symbol count. That is because Xcode's own default `STRIP_STYLE=all` for an app executable
is what a plain `xcrun strip` applies too, so `app_size.sh`'s stripped-Runner row now uses a
full strip, not `-x -S`: `-x -S` only removes local symbols and debug info, keeping every
global/exported one, so it understates the savings (10,141,104 B on this same plain build —
~1.6 MB above both the full strip and the real archive).

**`xcodebuild archive` does strip the Runner**, via its own deployment-postprocessing pass,
regardless of this project's `COPY_PHASE_STRIP = NO` and absent `DEPLOYMENT_POSTPROCESSING`
override — that project setting only governs a plain `xcodebuild build`/`frust build ios
--release`, which really does ship unstripped if used directly (e.g. sideloading via
`devicectl`), not the App Store archive route. **No Xcode template/product change follows
from this**: the artifact that actually reaches users (an App Store or ad-hoc `.ipa` built
via `archive` + export) was never the unstripped one this investigation worried about — a
plain `xcodebuild build`/`frust build ios --release` output (e.g. a `devicectl`-sideloaded
development build) was, and it remains genuinely unstripped.

**Attribution stays Android-only.** `size_attribute.py` reads ELF (`llvm-readelf` sections,
`llvm-nm` symbols) and does not parse Mach-O, so the iOS split above comes from
`otool -l`/`size -m` and carries no per-crate byte attribution.

**Size attribution (re-run, de-duplicated).** `size_attribute.py --nm-dir
<ndk bin>` on this pass's unstripped lean arm64 `.so` (15,728,352 B
unstripped; 6,812,544 B allocated/`SHF_ALLOC`): attributed total 4,623,257 B,
led by `core` 498,008 B, `naga` 414,759 B, `harfrust` 332,786 B, `wgpu_core`
321,020 B, `read_fonts` 313,648 B and `skrifa` 288,192 B; `sqlite3` does not
appear (this is the `db`-off/lean build). These rows replace the previously
published pre-dedup ones (`naga` 890,515 B, attributed total 7,406,938 →
7,406,834 B, `core` 660,128 B, `wgpu_core` 550,868 B, `sqlite3` 671,557 B) —
the de-duplicated tool attributes each `(address, size)` region once, so the
old and new rows are not comparable byte-for-byte.

Frust stays the smaller artifact on every axis it shares with Flutter. The
`db` feature gate is still the largest single lever available to an app: the
arm64 `.so` drops 8,837,776 → 6,809,224 B (−2,028,552 B, −22.9 %) with `db`
off, on top of everything the font/link-flag/opt-level/pin levers above
already removed from both builds.

## Renderer transition (vello → frust-engine) — regression check, 2026-09-05

Recorded once, on the 2026-09-05 pass: `harness/compare.py` over two `summarize.py` JSON dumps, the retired vello series (git history, `git show f64be636:benchmarks/raw/…`) against that pass's raws; Δ negative = faster. The same-build-mode (`--profile`) vello rows were the renderer comparison proper, the 2026-07-21 release rows cross-methodology (PROTOCOL §2.5, tails only). The full per-device comparison tables were folded into this summary on 2026-09-06, when the post-optimization pass above replaced the device sections they were computed against; they remain in git history (`git show de265855:benchmarks/RESULTS.md`).

**Verdict: no per-frame renderer regression on Android or iOS in the like-for-like rows.** Its two open measurement caveats — S5 content on the OnePlus 9 and S3 per-op attribution — are settled by the 2026-09-06 pass above; its iOS tail items are carried into that pass's criteria section.

- **Same build mode, the renderer comparison proper** (vello `--profile` 2026-09-01 → engine `--profile` 2026-09-05). OnePlus 9: S1 p50 14.84→8.20 ms (−45 %), S2 17.99→8.83 (−51 %), S5 22.07→7.78 (−65 %), S6 flat, every cell moving from 47–68 fps to a locked 120 fps. Pixel 5 (unpinned): the largest gains of that matrix — S1 48.40→14.86 (−69 %), S2 42.96→8.91 (−79 %), S5 95.56→13.19 (−86 %), S6 23.23→6.82 (−71 %), with every vello cell missing the 60 Hz budget on 100 % of frames against the engine's 0.1–1.7 %. iPhone SE: the 60 Hz cadence identical, per-frame cost fully observable at CPU 0.14–2.7 ms plus GPU 4.4 / 3.1 / 1.6 ms for S1 / S2 / S6; its "CPU work" row rose +8…+22 % only because the engine's `submit_us` waits on the GPU inside it while vello's compute was asynchronous and never measured — bookkeeping. Engine-to-engine drift was −3…−10 % (OnePlus 9) and within ±5 % (Pixel 5).
- **Cross-methodology** (vello RELEASE 2026-07-21 → engine profile, OnePlus 9): S1/S2/S5 tails −22…−56 %, S3 per-op −10…−22 %, cold start 185→135 ms, first frame 128→105 ms. The two rows that moved the wrong way are §2.5 artefacts: S6 p50 5.67→8.19 ms is the 120 Hz pin (period 8.33 ms; the same-build row is flat) and S8 writes 65–67→94–95 µs are plugin calls, not renderer work. Flutter's own 3.44.2→3.47.2 drift stayed <7 %, so the cross-app verdicts are not a Flutter artefact.
- **Headline tier, cross-methodology** (vello RELEASE 2026-07-21 → engine profile 2026-09-06, Xiaomi 12, added when that device was re-run): see its section — the engine holds 120.5–120.8 fps on every frame scenario where vello dropped to 65–89 fps on S2/S5/S6, with p95 down 14–45 % there and S1's median rise the same submit/GPU-wait bookkeeping as the iPhone SE row.

## DB scenarios (`d1`/`d2`) — no runs recorded yet

`PROTOCOL.md` §9 specifies the `d*` op-latency class (`d1` batched/autocommit
writes, `d2` point SELECT + range scan) against the fixed row shape and seed
dataset of §9.2, on §7's per-op raw-line contract. **No device has run it.**
When a pass is captured this section gains one subsection per device with a
`d1` and a `d2` table (per-op p50/p95/p99 µs + ops/s) across three columns —
Frust `frust-database` (in-process `rusqlite`), Flutter `package:sqlite3`
(engine-parity FFI) and Flutter `sqflite` (platform channel) — each side's
linked SQLite version recorded per run, plus a methodology-deviations list.
