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
device's delta table ("vs 2026-09-06" on the OnePlus 9, Pixel 5 and Xiaomi 12; "vs 2026-09-05" on the iPhone SE) is `harness/compare.py` over two of those JSON
dumps, both regenerated from the committed raw trees; Δ positive = slower. Prose mentions of an
earlier pass's own idle-memory figure are likewise recomputed from that pass's raw series in git
history, not copied from its published text (`de265855` Pixel 5, `32597904` OnePlus 9, `20e0aa2e`
Xiaomi 12).

> **Frust-fast close-out pass, 2026-10-17 (OnePlus 9, Pixel 5 and Xiaomi 12).** Every Frust series in
> the OnePlus 9 and Pixel 5 sections is a `--profile` build of `87271063` — `main` after the engine fast
> paths (paint-class programs, glyph quads, the resident glyph-run cache, the parallel strip walk, the
> staging ring and bind-group cache), the Android thread policy and gated JNI polls, and the
> first-frame warm-up and pipeline-cache persistence — against a Flutter 3.47.2 profile build; the
> Xiaomi 12 section is a `--profile` build of `20e0aa2e`, the same engine, render, GPU, shell and widget
> crates with `frust-core`'s readability refactor (PR #83) on top (its deviation 2). It replaces each of
> those three devices' 2026-09-06 series under `benchmarks/raw/` (PROTOCOL §2.6; the 2026-09-06 sections
> are in git history, `git show 87271063:benchmarks/RESULTS.md` for the two phones, `git show
> 20e0aa2e:benchmarks/RESULTS.md` for the Xiaomi 12) and each section carries a delta table against that
> pass for both apps. Each phone ran the full 16 blocks in one unattended `matrix.sh` session: OnePlus 9
> (USB, 120 Hz pinned, 07:24–11:04 UTC) and Pixel 5 (adb-over-Wi-Fi, unpinned, on battery, 07:24–10:54
> UTC) driven from a Linux host for the first time, Xiaomi 12 (USB, 120 Hz pinned, 17:08–19:07 UTC) from
> the Mac. The iPhone SE section still carries the 2026-09-06 pass.
> Toolchain for this pass: Rust 1.98.1 (pinned), Flutter 3.47.2 stable / Dart 3.13.2 (profile APK
> rebuilt on the Linux host for the two phones, on the Mac for the Xiaomi 12), `wgpu` 30.0.1 (pinned).

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

## Success criteria — frust-fast close-out pass, 2026-10-17 (OnePlus 9, Pixel 5, Xiaomi 12)

The frust-fast plan's criteria that these three devices can score, each with the number from this pass. The iPhone SE
rows stay open until its leg runs. Misses are recorded as misses.

| Criterion | Measured this pass | Verdict |
|---|---|---|
| OnePlus 9 S1 `gpu_main` ≤ 3.5 ms (from 6.37) | **3.47 ms** p50 over 35,996 `gpu_q=1` frames | **MET** |
| Pixel 5 S1 `gpu_main` ≤ 8.0 ms (from 13.60), Frust S1 median at or below Flutter's | **8.08 ms** p50 (GPU total; 26,512 frames); Frust S1 p50 **4.67** vs Flutter **10.10** | **MISS by 0.08 ms** on the GPU bar; the median bar **MET** |
| OnePlus 9 S4 p50 ≤ 4.3 ms and p95 ≤ 5.0 (from 4.77 / 6.17; Flutter 4.22 / 4.63) | **4.45 / 6.43** (Flutter 4.25 / 4.66) | **MISS** on both |
| Pixel 5 S4 p95 ≤ 7.5 ms (from 11.44; Flutter 6.74) | **6.69** (Flutter 5.64) | **MET** |
| Pixel 5 S5 p50 ≤ 8.4 ms (from 13.04; Flutter 8.39) | **5.86** (Flutter 6.20) | **MET** |
| OnePlus 9 S5 p50 ≤ 6.4 ms (from 7.93; Flutter 6.43) | **6.20** (Flutter 6.59) | **MET** |
| S6 no regression elsewhere; Android S2 unchanged or better | OnePlus 9 S6 8.44→5.01, Pixel 5 S6 p50 7.88→6.22 but p95 10.15→**12.68**; S2 9.06→6.37 and 10.18→4.70 | **MET on p50**; Pixel 5 S6 p95 **worse** |
| Android `first_encode_done` → `first_frame_presented` ≤ 25 ms on a warm launch | OnePlus 9 **10.5 ms** median [10–12], Pixel 5 **13.0** [12–16] (2026-09-06: 53.5 and 123.5) | **MET** |
| No regression: S3 and S8 rows, idle PSS | idle PSS 114.1→82.5 and 94.2→85.0 MiB; S8 writes −1…−24 % / reads flat on the OnePlus 9, writes −7…−17 % on the Pixel 5; S3 overall p50 −24 % / −30 % and every Pixel 5 op faster, **but OnePlus 9 `create 1k` 14.98→21.38 ms p50 (+43 %)** | **MET** except the OnePlus 9 S3 `create 1k` op |
| Final matrix: Frust at or below Flutter's p50 on every S1–S8 row, or the exceptions named | Pixel 5: every row; OnePlus 9: every row except **S4 (4.45 vs 4.25)**. On both devices the framework-reported first-frame span is longer than Flutter's (114 vs 74, 202 vs 162 ms) — different entry points, as on every pass, and now also an `adapter_ready` regression (below) | **MET with two named exceptions** |
| Xiaomi 12 S1 GPU pass time (no bar set for this device) | `gpu_q=1` total p50 **4.58 ms** over 36,159 frames (2026-09-06: 5.25; S1 p50 6.58 vs Flutter 9.52) | **reported** (−13 %) |
| Xiaomi 12 `first_encode_done` → `first_frame_presented` ≤ 25 ms on a warm launch | **9.0 ms** median [8–11] over 12 launches (2026-09-06: 53.5); `adapter_ready` 35 → **107** ms, `first_frame_presented` 97 → 123, `am start -W` 107 → 99 | **MET**; the `adapter_ready` move is on all three Android devices |
| Xiaomi 12 no regression: S3 and S8 rows, idle PSS | idle PSS 54.9 → 48.5 MiB; S8 writes −10…+4 %, reads +1 µs; **every S3 reconcile op slower: `create 1k` 12.37 → 22.46 ms p50 (+82 %), `create 10k` +59 %, `clear` +61 %, `update` +42 %, `swap` +36 %** (overall series p50 −15 %, p95 +42 %) | **MISS on S3** (all five ops); MET on S8 and PSS |
| Xiaomi 12 final matrix: Frust at or below Flutter's p50 on every S1–S8 row, or the exceptions named | every row except **S4 (4.17 vs 4.11)** and **S5 (5.75 vs 5.59)**; 2026-09-06 had the S5 median (6.57 vs 5.93) and the S4 tail with Flutter. First-frame span 123 vs 64 ms (different entry points, as above) | **MET with two named exceptions** |

**The first frame after encode is fixed; the span before it moved the other way.** `first_encode_done` → `first_frame_presented`
fell from 53.5 to 10.5 ms (OnePlus 9) and from 123.5 to 13.0 ms (Pixel 5) — the pipeline-cache blob now holds the engine
pipelines (`PERFORMANCE_BASELINES.md` § Engine close-out per device: 117 KB, 18 pipelines, against 328 B and none) and the first
frame steals only the variants it draws. But `adapter_ready` (the stamp after adapter, device and renderer creation, which also
covers seeding the driver cache from that blob and queueing the warm-up) rose from 36 to 100 ms on the OnePlus 9 and from 44 to 180 ms
on the Pixel 5, so the whole `first_frame_presented` span is **longer** than on 2026-09-06 (95→114 ms and 172→202 ms) while
`am start -W` is flat (128→128) or better (159→141). The two stamps bound different work; which of the intervening changes
(driver-cache seeding, the hardware-buffer device probe, the warm-up worker starting beside device creation) owns the extra
device-open time is not measured here and is the open item this pass hands back. The Xiaomi 12 repeats the
pattern from the Mac: 53.5 → 9.0 ms after encode, `adapter_ready` 35 → 107 ms, `first_frame_presented` 97 → 123,
`am start -W` 107 → 99 — three devices, one direction.

**Xiaomi 12: every S3 reconcile op got slower, and on four of the five it is rebuild.** `create 1k` 12.37 → 22.46 ms p50,
`create 10k` 12.93 → 20.52, `clear` 10.34 → 16.66, `update` 10.73 → 15.22, `swap` 10.22 → 13.91. On the last four the
whole move is the UI half — the op frames' rebuild median roughly doubled (4.89–7.64 → 10.04–16.21 ms) while submit got
cheaper; on `create 1k` it is submit (9.12 → 18.23 ms), the shape the OnePlus 9 shows. The Xiaomi ran `20e0aa2e`, which
differs from the two phones' `87271063` only in `frust-core` (the readability refactor, PR #83), and the OnePlus 9 on
`87271063` shows no rebuild move (`clear` 15.18 → 15.33), so an A/B of the two commits on this device is the second open
item this pass hands back.

---

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

**Status:** run 2026-10-17 07:24–10:54 UTC, **12 runs × 30 s per scenario per app (S7: 60 s runs), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** **Frust = PROFILE build of `87271063`** (`frust build … --profile --define FRUST_TRACE_RAW=1`, raw format v4), **Flutter = profile build** (`flutter build … --profile`, Flutter 3.47.2). Raw series under `raw/pixel5/frust_profile/<sN>/` and `raw/pixel5/flutter/<sN>/` (run-NN.log + run-NN.pss_before/after.txt, stats.txt; S7 adds cpuinfo.txt/coldstart.txt). One unattended `matrix.sh` session over adb-over-Wi-Fi; every block captured, the driver graded all 14 frame/plugin blocks `DEGENERATE` on this Linux host and re-ran each once (deviation 2), so the session ran 30 blocks; the published series are the first attempts except flutter S5 (deviation 3). **This pass supersedes the 2026-09-06 pass on this device** (its section: `git show 87271063:benchmarks/RESULTS.md`).

- Chipset: Snapdragon 765G (board `lito`) / Adreno 620, GLES driver V@0490.0. Model: Pixel 5 `redfin` (serial redacted).
- OS: Android 14, `UP1A.231105.001.B2`, SDK 34 (`google/redfin/redfin:14/UP1A.231105.001.B2/11260668:user/release-keys`).
- Display: 1080×2340 @ ~435 dpi; modes id 0 = 60 Hz, id 1 = 90 Hz. **Refresh NOT pinned** (`peak_refresh_rate`/`min_refresh_rate` unset throughout, matching this device's earlier passes); the apps opt in themselves. The panel was in the 90 Hz mode at session start and end (`mActiveSfDisplayMode` id 1), as on 2026-09-06. stats.py budgets are 16.67 / 8.33 ms; the 11.11 ms 90 Hz budget is not a column.
- High-refresh opt-in engaged on both apps (§6). Achieved (active frames ÷ 300 s): Frust ~88.5–88.8 fps in every frame scenario (2026-09-06: ~65.9 in S1 and ~79.4 in S5); Flutter ~82.7 (S1), ~87.1 (S5), ~87.8–87.9 (S4/S6); S2/S3 Flutter paint is event-driven (~14.2 / ~3.1 fps).
- Brightness fixed 128/255, auto-brightness off (`device_state.sh`) for every block; screen timeout 30 min, `svc power stayon true` during blocks; battery saver off.
- Radios: **left exactly as found by `--wireless`** (airplane setting untouched, Wi-Fi up, a tunnel carrying the adb link) — not the §3 radios-off state (deviation 1).
- Charger: **on battery for the whole session** — `AC powered: false`, `USB powered: false` at both ends, real level 93 % at the first block and 51 % at the last (2026-09-06 opened on AC at 100 % and went to battery mid-session); `dumpsys battery unplug` spoofed on-battery state before every block and the `--min-level 30` guard never waited.
- Thermal: gate ceiling 38 °C / 120 s cooldown before every block. Session start 26.3 °C, peak 30.2 °C (after the second flutter S5 block), end 28.0 °C. **No cooldown stall in any block.** 30 s blocks 384–389 s wall, 60 s S7 blocks 745 s.
- Toolchain: Rust 1.98.1 (pinned), `wgpu` 30.0.1 (pinned); Flutter 3.47.2 stable / Dart 3.13.2. Frust APK arm64-only, 10,100,171 B (md5 8492040f…); Flutter APK fat profile build rebuilt on the Linux host, 69,869,386 B (md5 abc986a2…) — not a size comparison. Both installed fresh by the driver; md5s are the host's (the driver's read-back is empty on Linux).
- Visual gate (driver screencap per block, all 30 reviewed by band luma plus spot checks): **content PASS on every published block, both apps**, and no keyboard or other system window intruded on any capture (bottom-band luma 19–124 across all 30). The first flutter S5 attempt's capture showed four undecoded placeholder cells and no decoded image, so its retry — two decoded cells over placeholders, the 2026-09-06 picture — is the published flutter S5 series (deviation 3); Frust's S5 captures show the decoded column (mid-band luma 116–123).
- Sanitization: staged by hand from the driver's per-attempt trees with the driver's own copy rules and self-checks (`RAW_OK`: 192 run logs, 384 PSS snapshots, 16 stats.txt, S7 cpuinfo/coldstart), whitelist self-check passed, no address or serial in any staged file.

### S1

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.67 | 6.99 | 9.11 | 47.88 | 76 (0.29%) | 516 (1.94%) | 26,552 (~88.5 fps avg) |
| Flutter (profile) | 10.10 | 16.09 | 22.65 | 69.20 | 1,143 (4.61%) | 23,917 (96.40%) | 24,809 (~82.7 fps avg) |

Frust pass p50 (ms): rebuild 0.08, layout 0.00, paint 0.12, encode 0.09, acquire 0.06, submit 4.12; layout_us>0 on 26,552/26,552 frames; GPU (gpu_q=1 on 26,512) total p50/p95 8.08/8.29 ms

**Frust wins S1 on every statistic.** p50 4.67 vs 10.10 ms, p95 6.99 vs 16.09, p99 9.11 vs 22.65, worst 47.88 vs 69.20, and 0.29 % vs 4.61 % of frames over the 60 Hz budget, at ~88.5 vs ~82.7 fps. On 2026-09-06 this device was the one where Frust lost S1 (15.23 vs 12.55 ms at ~65.9 fps); the GPU pass time under the frame is now 8.08 ms p50 against 13.60 then — 0.08 ms over the plan's 8.0 ms bar, with the median bar (at or below Flutter's) met by a wide margin.

### S2

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.70 | 8.25 | 11.60 | 26.01 | 26 (0.10%) | 1,247 (4.68%) | 26,617 (~88.7 fps avg) |
| Flutter (profile) | 21.02 | 27.87 | 49.38 | 77.17 | 4,117 (96.35%) | 4,236 (99.13%) | 4,273 (~14.2 fps avg) |

Frust pass p50 (ms): rebuild 0.07, layout 1.55, paint 0.17, encode 0.06, acquire 0.08, submit 2.81; layout_us>0 on 26,617/26,617 frames; GPU (gpu_q=1 on 26,577) total p50/p95 5.83/6.06 ms

**Frust wins S2 decisively** — p50 4.70 vs 21.02 ms, p95 8.25 vs 27.87, 0.10 % vs 96.35 % over the 60 Hz budget, ~88.7 fps continuous against Flutter's one-shot script (~14.2 fps event-driven over the block). Frust's own S2 median fell 10.18 → 4.70 ms (layout 1.55 ms of it, GPU 5.83).

### S4

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 3.94 | 6.69 | 7.32 | 31.04 | 12 (0.05%) | 37 (0.14%) | 26,644 (~88.8 fps avg) |
| Flutter (profile) | 4.30 | 5.64 | 6.68 | 29.43 | 6 (0.02%) | 43 (0.16%) | 26,371 (~87.9 fps avg) |

Frust pass p50 (ms): rebuild 0.10, layout 0.01, paint 0.05, encode 0.02, acquire 1.54, submit 2.06; layout_us>0 on 26,644/26,644 frames; GPU (gpu_q=1 on 26,604) total p50/p95 2.62/2.88 ms

**Split, and the median changed sides.** Frust takes p50 (3.94 vs 4.30 ms; on 2026-09-06 Flutter led 5.17 vs 5.93) and the worst frame is a tie (31.04 vs 29.43); Flutter keeps the tail (p95 5.64 vs 6.69, p99 6.68 vs 7.32) at the same ~88 fps and the same ~0 % miss rate. Frust's S4 p95 fell 11.44 → 6.69 ms, meeting the plan's 7.5 ms bar.

### S5

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 5.86 | 12.12 | 13.30 | 29.13 | 37 (0.14%) | 5,374 (20.20%) | 26,600 (~88.7 fps avg) |
| Flutter (profile) | 6.20 | 10.53 | 12.97 | 25.69 | 15 (0.06%) | 4,832 (18.49%) | 26,134 (~87.1 fps avg) |

Frust pass p50 (ms): rebuild 0.45, layout 0.01, paint 0.04, encode 0.02, acquire 1.89, submit 3.06; layout_us>0 on 26,600/26,600 frames; GPU (gpu_q=1 on 26,560) total p50/p95 8.82/9.27 ms

**Split, with the median to Frust.** p50 5.86 vs 6.20 ms (2026-09-06: 13.04 vs 8.39 — Frust was 55 % slower than Flutter here and is now ahead), the 60 Hz miss rate 0.14 % vs 0.06 %; Flutter wins p95 (10.53 vs 12.12) and p99 (12.95 vs 13.30) at the same ~88 fps. GPU pass time 8.82 ms p50 is the biggest share of Frust's frame on this scene. The published flutter series is the block's retry (deviation 3); its first attempt measured the same (p50 6.17, p95 10.53).

### S6

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 6.22 | 12.68 | 13.82 | 29.70 | 31 (0.12%) | 11,543 (43.36%) | 26,624 (~88.7 fps avg) |
| Flutter (profile) | 6.67 | 10.83 | 11.22 | 23.40 | 21 (0.08%) | 11,573 (43.94%) | 26,338 (~87.8 fps avg) |

Frust pass p50 (ms): rebuild 0.09, layout 0.14, paint 0.18, encode 0.10, acquire 2.55, submit 2.38; layout_us>0 on 26,624/26,624 frames; GPU (gpu_q=1 on 26,584) total p50/p95 3.70/4.05 ms

**Split, with the median to Frust and the tail to Flutter.** p50 6.22 vs 6.67 ms (2026-09-06: 7.88 vs 10.25), but p95 12.68 vs 10.83, p99 13.82 vs 11.22 and worst 29.70 vs 23.40 go to Flutter, both at ~88 fps with ~0.1 % misses. Frust's S6 p95 is the one frame-table line on this device that is **worse than September** (10.15 → 12.68); the median improved 21 %.

### S3

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | 8.92 / 21.51 / 22.47 / 23.94 | 266 | 5.77 / 7.08 / 7.08 / 7.08 | 8 |
| create 10k | 7.14 / 13.62 / 14.47 / 19.46 | 270 | **not captured** | 0 |
| update every 10th of 10k | 5.76 / 9.84 / 10.51 / 11.07 | 270 | **not captured** | 0 |
| swap | 5.26 / 9.25 / 9.78 / 10.44 | 270 | **not captured** | 0 |
| clear | 5.83 / 10.51 / 11.74 / 18.92 | 264 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.61 | 8.24 | 11.13 | 23.94 | 60 (0.23%) | 1,269 (4.76%) | 26,634 (~88.8 fps avg) |
| Flutter (profile) | 11.79 | 49.66 | 53.59 | 57.16 | 209 (22.69%) | 734 (79.70%) | 921 (~3.1 fps avg) |

Cycle health — Frust (profile) `s3-create1k` reopens per kept run: 27–27; Flutter (profile) `s3-create1k` reopens per kept run: 19–19.

**Frust wins S3 on every comparable axis, and every reconcile op got faster.** Overall series p50 4.61 vs 11.79 ms at ~88.8 vs ~3.1 fps; the five Frust ops land 264–270 frames per kept series and moved −24…−38 % on p50 against 2026-09-06 (create 1k 8.92, create 10k 7.14, update 5.76, swap 5.26, clear 5.83 ms), with `clear` and `create 1k` p95 up 14 % / 11 %. Flutter's `create 1k` lands 8 frames (5.77 ms); the other ops are not captured on the Flutter side, as before.

### S7

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | ~141 ms (128/142/141; first of 4 discarded) | ~643 ms (615/668/643; first of 4 discarded) |
| Framework-reported first-frame span | ~202 ms median (`first_frame_presented`, 185–220 across 12 launches; `adapter_ready` median 179 ms) | ~162 ms median (`first_frame_ms`, 155–171 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | ~7.6% avg (max 7.6%, 1 samples) | 0 samples |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~85.0 MiB (84.8–85.3, 10 kept snapshots) | ~115.0 MiB (112.8–122.6, 10 kept snapshots) |

**Frust starts ~4.6× faster and idles ~26 % lighter; its framework-reported first frame got longer.** External cold start 141 vs 643 ms (`am start -W` TotalTime; 159 on 2026-09-06), idle TOTAL PSS 85.0 vs 115.0 MiB (94.2 before). Frust's first-frame span is 202 ms median (172 before) against Flutter's 162 (unchanged), from different entry points; inside it `first_encode_done` → `first_frame_presented` fell 123.5 → 13.0 ms [12–16] while `adapter_ready` rose 44 → 180 ms (criteria block above). Idle CPU: Frust appeared in one `dumpsys cpuinfo` sample at 7.6 % during its S7 block, Flutter in none (2026-09-06: 0 and 0).

### S8

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (profile) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 127 | 392 | n/a |
| write i64 | 128 | 408 | n/a |
| write f64 | 134 | 452 | n/a |
| write String | 130 | 403 | n/a |
| write Vec\<String\> | 137 | 419 | n/a |
| read (unique key, forces channel) bool | 41 | 11973† | ~1 |
| read (unique key, forces channel) i64 | 42 | 11973† | ~1 |
| read (unique key, forces channel) f64 | 42 | 11973† | ~1 |
| read (unique key, forces channel) String | 40 | 11973† | ~1 |
| read (unique key, forces channel) Vec\<String\> | 41 | 11973† | ~2 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust wins S8 decisively** — writes 127–137 vs 392–452 µs/call (~3.0–3.4×; Frust's writes fell 7–17 % since 2026-09-06), a per-key read 40–42 µs against Flutter's whole-store `reload()` at 11,973 µs. Zero `s8-errors` on either app.

### vs 2026-09-06 (same device, same scenarios)

Frust, Δ positive = slower / worse:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|---|---|
| S1 | 15.23→4.67 | -69.4% | 18.03→6.99 | -61.2% | 19.00→9.11 | 146.39→47.88 | 10.20%→0.29% | 65.9→88.5 |
| S2 | 10.18→4.70 | -53.8% | 11.32→8.25 | -27.1% | 14.14→11.60 | 158.28→26.01 | 0.51%→0.10% | 88.4→88.7 |
| S4 | 5.93→3.94 | -33.5% | 11.44→6.69 | -41.5% | 12.45→7.32 | 148.06→31.04 | 0.17%→0.05% | 88.6→88.8 |
| S5 | 13.04→5.86 | -55.1% | 13.84→12.12 | -12.4% | 14.62→13.30 | 164.75→29.13 | 0.45%→0.14% | 79.4→88.7 |
| S6 | 7.88→6.22 | -21.1% | 10.15→12.68 | +24.9% | 13.43→13.82 | 166.62→29.70 | 0.37%→0.12% | 88.4→88.7 |
| S3 (overall series) | 6.61→4.61 | -30.2% | 10.84→8.24 | -23.9% | 13.21→11.13 | 144.59→23.94 | 0.53%→0.23% | n/a→n/a |

Frust S3 per-op (reconcile frames):

| Op | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | n frames |
|---|---|---|---|---|---|
| s3-clear | 7.67→5.83 | -23.9% | 9.24→10.51 | +13.8% | 260→264 |
| s3-create10k | 9.85→7.14 | -27.5% | 14.21→13.62 | -4.1% | 270→270 |
| s3-create1k | 12.70→8.92 | -29.8% | 19.36→21.51 | +11.1% | 260→266 |
| s3-swap | 8.47→5.26 | -37.9% | 10.42→9.25 | -11.2% | 270→270 |
| s3-update | 8.57→5.76 | -32.8% | 10.59→9.84 | -7.1% | 269→270 |

Frust S7:

| Metric | old | new | Δ |
|---|---|---|---|
| first-frame span median (ms) | 172 | 202 | +17.4% |
| idle TOTAL PSS mean (MiB) | 94.2 | 85.0 | -9.8% |
| external cold start median (ms) | 159 | 141 | -11.3% |

Flutter control on the same session:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|---|---|
| S1 | 12.55→10.10 | -19.5% | 20.21→16.09 | -20.4% | 23.54→22.65 | 68.05→69.20 | 5.83%→4.61% | 82.0→82.7 |
| S2 | 31.28→21.02 | -32.8% | 40.59→27.87 | -31.3% | 66.64→49.38 | 83.79→77.17 | 99.49%→96.35% | 10.6→14.2 |
| S4 | 5.17→4.30 | -16.8% | 6.74→5.64 | -16.4% | 8.11→6.68 | 29.48→29.43 | 0.02%→0.02% | 87.8→87.9 |
| S5 | 8.39→6.20 | -26.2% | 12.57→10.53 | -16.2% | 15.37→12.97 | 30.05→25.69 | 0.42%→0.06% | 84.0→87.1 |
| S6 | 10.25→6.67 | -35.0% | 17.96→10.83 | -39.7% | 19.65→11.22 | 37.32→23.40 | 9.39%→0.08% | 87.5→87.8 |
| S3 (overall series) | 19.34→11.79 | -39.0% | 54.08→49.66 | -8.2% | 59.12→53.59 | 67.91→57.16 | 52.40%→22.69% | n/a→n/a |

**Read the Frust deltas against a control that moved too.** The Flutter control improved −16.8…−39.0 % on p50 across every frame scenario on this session (S1 12.55→10.10, S6 10.25→6.67) with its S7 first frame flat (162→162) and cold start flat (638→643), so a large part of the Pixel 5's between-session shift is environmental — the September session ran from 100 % on AC into battery with a 65.9 fps Frust S1, this one on battery throughout at 26–30 °C, and the two Flutter APKs were built on different hosts. Frust's moves are larger than the control's on every row (S1 −69.4 %, S2 −53.8 %, S4 −33.5 %, S5 −55.1 %, S6 −21.1 %, S3 −30.2 % on p50) and its per-frame GPU time is a measured engine change (S1 13.60 → 8.08 ms), but the exact split between engine work and session state is not recoverable from this pass; Frust S6 p95 (+24.9 %) and the S7 first-frame span (+17.4 %) are the two lines that got worse.

### Methodology deviations (this device)

1. **adb-over-Wi-Fi, radios left as found.** The device was reached over `--wireless`, so airplane mode, Wi-Fi and Bluetooth were not touched (toggling them would cut the link) and the §3 radios-off state was not applied; the link ran through a tunnel the whole session. The session ran on battery from 93 % to 51 % with no charger (2026-09-06 opened on AC); the `--min-level 30` guard never waited.
2. **Driver grading failed on the Linux host; every frame/plugin block was re-run once.** As on the OnePlus 9 this session: `block_sanity`'s grep pattern begins with `-` and GNU grep rejects it, so every block graded `DEGENERATE` and was retried, and the published series were staged by hand from the driver's per-attempt trees (first attempt of each block except flutter S5). The data under every attempt is complete (2,500–2,700 raw lines per 30 s run); the thermal headroom on this device (26–30 °C) absorbed the doubled load with no cooldown stall.
3. **flutter S5 is the second attempt.** The first attempt's screencap showed four grey placeholder cells and no decoded image, which fails the content gate the 2026-09-06 pass applied (three decoded cells and one placeholder); the retry's capture shows two decoded cells over placeholders and is the published series. The two attempts measured the same (p50 6.17 / 6.20 ms, p95 10.53 / 10.53), so the swap changes the picture, not the number. Frust's S5 series is the first attempt (p50 5.86; its retry read 6.71).
4. **The Flutter control moved −17…−39 % on p50 against 2026-09-06** while the OnePlus 9's control on the same host and APK moved ±2.5 % — the Pixel 5's September session is the outlier (Frust S1 at ~65.9 fps there), and this device's deltas are between-session comparisons, not clean A/Bs (see the delta note).
5. **Flutter APK rebuilt on the Linux host** with the same Flutter 3.47.2 / Dart 3.13.2 (69,869,386 B against 70,462,089 B on the Mac); the driver's install log carries no md5 on Linux (`md5 -q`), the md5s in the facts are the host's.
6. **Flutter's S2 script runs once per launch** while Frust's loops — a declared asymmetry in both scenario sources (460 Flutter S2 raw frames per 30 s run this pass).
7. **Frust `adapter_ready` 44 → 180 ms** (`first_frame_presented` 172 → 202) while the post-encode first frame fell 123.5 → 13.0 ms — the open item in the criteria block.
8. **S7 idle CPU: Frust appeared in one `dumpsys cpuinfo` sample (7.6 %)** during its S7 block, where 2026-09-06 had 0 samples on both sides; Flutter 0 samples. One sample in a 60 s window is not a rate and the row carries it as such.
9. As on prior passes: Flutter release-mode in-app cross-check not captured; S4 parse wall time not recoverable; S8 burst-during-animation variant not run; the APK packaging asymmetry is not a size comparison; no `img` line is emitted on this device (the 2026-09-06 S5 content note stands).

---

## Device: OnePlus 9 (LE2115 "lemonade", Snapdragon 888 / Adreno 660) — mid-tier Android

**Status:** run 2026-10-17 07:24–11:04 UTC, **12 runs × 30 s per scenario per app (S7: 60 s runs), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** **Frust = PROFILE build of `87271063`** (`frust build … --profile --define FRUST_TRACE_RAW=1`, raw format v4; the engine fast paths, thread policy, gated JNI polls, first-frame warm-up and pipeline-cache persistence), **Flutter = profile build** (`flutter build … --profile`, Flutter 3.47.2). Raw series under `raw/oneplus9/frust_profile/<sN>/` and `raw/oneplus9/flutter/<sN>/` (run-NN.log + run-NN.pss_before/after.txt, stats.txt; S7 adds cpuinfo.txt/coldstart.txt). One unattended `matrix.sh` session; every block captured, but the driver graded all 14 frame/plugin blocks `DEGENERATE` on this Linux host and re-ran each once (deviation 1), so the session ran 30 blocks and the published series are the first attempts except frust S1 (deviation 2). **This pass supersedes the 2026-09-06 pass on this device** (its section: `git show 87271063:benchmarks/RESULTS.md`).

- Chipset: Snapdragon 888 (board `lahaina`) / Adreno 660, GLES driver V@0530.53. Model: OnePlus 9 LE2115 (serial redacted).
- OS: LineageOS — Android 15, `lineage_lemonade-userdebug 15 BP1A.250405.007` (nightly 20250405), SDK 35; fingerprint still reports `OnePlus9:14/UKQ1.230924.001`. Unchanged since 2026-09-06.
- Display: 1080×2400 @ ~392 dpi; modes id 0 = 60 Hz, id 1 = 120 Hz (active at session start this time). **Both `min_refresh_rate`/`peak_refresh_rate` pinned to 120 for every block** (`--pin-refresh 120`, read-back `peak=120.0 min=120.0`, restored to unset afterwards). Both budgets (16.67 / 8.33 ms) reported; the 8.33 ms column counts the two-thread cost sum, not vsync misses (the 2026-09-06 note still applies).
- High-refresh opt-in engaged on both apps (§6). Achieved (active frames ÷ 300 s): Frust ~120.0–120.1 fps in S1/S2/S4/S5/S6 and ~113.2 in S3; Flutter ~98.9 (S1), ~107.2 (S5), ~119.2 (S4/S6); S2/S3 Flutter paint is event-driven (~19.3 / ~3.1 fps).
- Brightness fixed 128/255, auto-brightness off; airplane on (read-back 1), Wi-Fi off, Bluetooth off, battery saver off, screen timeout 30 min, `svc power stayon true` during blocks; all restored at 11:04 UTC (airplane 0, Wi-Fi on, BT on, refresh pins unset).
- Charger: USB-connected throughout (adb); `dumpsys battery unplug` spoofed on-battery state before every block. Real level 100 % for all 30 blocks; the `--min-level 30` guard never waited.
- Thermal: gate ceiling 38 °C / 120 s cooldown before every block. Session start 32.1 °C, **peak 40.7 °C** (after the second flutter S5 block), end 36.6 °C. The doubled block count kept the phone above the ceiling from the second frust S5 block through the first flutter S6 block: **six cooldown timeouts** (frust S5 retry, both flutter S5 attempts, both frust S6 attempts, flutter S6 first attempt; 501–503 s wall against 380–382 s) plus one on the frust S7 block (862 s against 741 s), where 2026-09-06 had one (deviation 3). The published S5 and S6 series are the first attempts: frust S5 at 36.4→38.4 °C, flutter S5 at 39.6→40.4 °C, frust S6 at 40.7→39.3 °C, flutter S6 at 38.3→37.6 °C.
- Toolchain: Rust 1.98.1 (pinned), `wgpu` 30.0.1 (pinned); Flutter 3.47.2 stable / Dart 3.13.2. Frust APK arm64-only, 10,100,171 B (md5 8492040f…); Flutter APK fat profile build rebuilt on the Linux host, 69,869,386 B (md5 abc986a2…; 2026-09-06's Mac build was 70,462,089 B) — not a size comparison. Stale installs removed and both installed fresh by the driver; the driver's md5 read-back is empty on Linux (deviation 4), the md5s here are the host's.
- Visual gate (driver screencap per block, all 30 reviewed by band luma plus spot checks): **content PASS on every published block, both apps.** S5 shows the decoded image column on both apps (mid-band luma 110–119); no keyboard or other system window intruded on any capture (bottom-band luma 20–122 everywhere; the 2026-09-06 Gboard intrusion did not recur). The only dark capture is the discarded first frust S1 attempt (deviation 2).
- Sanitization: staged by hand from the driver's per-attempt trees with the driver's own copy rules and self-checks (`RAW_OK`: 192 run logs, 384 PSS snapshots, 16 stats.txt, S7 cpuinfo/coldstart), whitelist self-check passed, no address or serial in any staged file.

### S1

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 8.05 | 8.87 | 9.30 | 15.82 | 0 (0.00%) | 11,937 (33.12%) | 36,038 (~120.1 fps avg) |
| Flutter (profile) | 9.47 | 17.17 | 19.43 | 86.47 | 1,551 (5.23%) | 27,158 (91.52%) | 29,673 (~98.9 fps avg) |

Frust pass p50 (ms): rebuild 0.18, layout 0.01, paint 0.46, encode 0.09, acquire 1.56, submit 5.67; layout_us>0 on 36,038/36,038 frames; GPU (gpu_q=1 on 35,996) total p50/p95 3.47/3.49 ms

**Frust wins S1, now on every statistic.** p50 8.05 vs 9.47 ms, p95 8.87 vs 17.17, p99 9.30 vs 19.43, worst 15.82 vs 86.47, and 0.00 % vs 5.23 % of frames over the 60 Hz budget at ~120.1 vs ~98.9 fps. The GPU pass time under the frame halved against 2026-09-06 (total p50 6.45 → 3.47 ms): the paint-class programs took S1 off the fragment-bound floor this device sat on, which is the plan's S1 criterion (3.47 against the 3.5 ms bar).

### S2

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 6.37 | 9.55 | 12.48 | 24.44 | 34 (0.09%) | 3,816 (10.61%) | 35,971 (~119.9 fps avg) |
| Flutter (profile) | 15.61 | 24.96 | 28.40 | 83.03 | 2,305 (39.78%) | 5,158 (89.01%) | 5,795 (~19.3 fps avg) |

Frust pass p50 (ms): rebuild 0.08, layout 2.09, paint 0.31, encode 0.05, acquire 0.09, submit 3.63; layout_us>0 on 35,971/35,971 frames; GPU (gpu_q=1 on 35,931) total p50/p95 1.37/1.41 ms

**Frust wins S2 decisively** — p50 6.37 vs 15.61 ms, p95 9.55 vs 24.96, and 0.09 % vs 39.78 % of frames over the 60 Hz budget. Frust holds ~119.9 fps through its looped scroll script while Flutter's script runs once and its paint is event-driven afterwards (~19.3 fps over the block), as on every pass; Frust's own S2 median fell 9.06 → 6.37 ms with the resident glyph-run cache and the staging ring (layout 2.09 ms of the 6.37, GPU 1.37 ms).

### S4

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.45 | 6.43 | 7.48 | 37.18 | 17 (0.05%) | 118 (0.33%) | 35,997 (~120.0 fps avg) |
| Flutter (profile) | 4.25 | 4.66 | 5.81 | 68.78 | 1 (0.00%) | 2 (0.01%) | 35,760 (~119.2 fps avg) |

Frust pass p50 (ms): rebuild 0.12, layout 0.01, paint 0.08, encode 0.03, acquire 0.39, submit 3.75; layout_us>0 on 35,997/35,997 frames; GPU (gpu_q=1 on 35,957) total p50/p95 0.46/0.47 ms

**Flutter wins S4, by less than before.** p50 4.25 vs 4.45 ms, p95 4.66 vs 6.43, p99 5.81 vs 7.48, both at ~120 fps and both essentially never missing the 60 Hz budget (0.00 % vs 0.05 %); Frust's worst frame is the better one (37.18 vs 68.78). Frust moved 4.77 → 4.45 on the median since 2026-09-06 and stays 0.20 ms behind — the plan's S4 bars (4.3 / 5.0) are missed, and the measured thread policy was neutral on this row (`PERFORMANCE_BASELINES.md`).

### S5

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 6.20 | 6.94 | 7.57 | 15.86 | 0 (0.00%) | 158 (0.44%) | 36,008 (~120.0 fps avg) |
| Flutter (profile) | 6.59 | 14.37 | 16.61 | 67.36 | 310 (0.96%) | 11,177 (34.74%) | 32,169 (~107.2 fps avg) |

Frust pass p50 (ms): rebuild 0.71, layout 0.02, paint 0.08, encode 0.02, acquire 0.10, submit 5.21; layout_us>0 on 36,008/36,008 frames; GPU (gpu_q=1 on 35,968) total p50/p95 2.93/2.97 ms

**Frust wins S5 outright now.** On 2026-09-06 Flutter held the median (6.43 vs 7.93); this pass Frust takes it (6.20 vs 6.59) and keeps the tail (p95 6.94 vs 14.37, p99 7.57 vs 16.61), the 60 Hz miss rate (0.00 % vs 0.96 %) and the cadence (~120.0 vs ~107.2 fps). GPU pass time 2.93 ms p50 against 6.45 before. This block ran at the session's thermal peak (36.4 → 38.4 °C, flutter at 39.6 → 40.4 °C), so both apps paid the same hot phone (deviation 3).

### S6

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 5.01 | 8.81 | 10.11 | 25.23 | 2 (0.01%) | 2,578 (7.16%) | 36,024 (~120.1 fps avg) |
| Flutter (profile) | 8.20 | 9.79 | 10.31 | 86.37 | 3 (0.01%) | 16,241 (45.43%) | 35,752 (~119.2 fps avg) |

Frust pass p50 (ms): rebuild 0.12, layout 0.19, paint 0.35, encode 0.12, acquire 0.11, submit 4.02; layout_us>0 on 36,024/36,024 frames; GPU (gpu_q=1 on 35,984) total p50/p95 0.60/0.62 ms

**Frust wins S6, where 2026-09-06 was a near-tie.** p50 5.01 vs 8.20 ms (8.44 vs 8.30 before), p95 8.81 vs 9.79, worst 25.23 vs 86.37; the 60 Hz miss rates are both ~0.01 %. The resident glyph-run cache and the glyph-atlas replay fix took Frust's S6 median down 40.6 %, with GPU pass time at 0.60 ms.

### S3

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | 21.38 / 25.91 / 27.14 / 28.42 | 344 | 9.80 / 9.93 / 9.93 / 9.93 | 2 |
| create 10k | 19.37 / 20.38 / 20.67 / 21.13 | 350 | **not captured** | 0 |
| update every 10th of 10k | 14.35 / 16.46 / 17.18 / 19.15 | 350 | **not captured** | 0 |
| swap | 13.27 / 16.03 / 17.16 / 17.76 | 340 | **not captured** | 0 |
| clear | 15.33 / 18.01 / 18.85 / 20.28 | 340 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.32 | 12.63 | 19.80 | 28.42 | 759 (2.23%) | 2,187 (6.44%) | 33,964 (~113.2 fps avg) |
| Flutter (profile) | 19.04 | 33.73 | 35.19 | 40.11 | 617 (67.07%) | 910 (98.91%) | 920 (~3.1 fps avg) |

Cycle health — Frust (profile) `s3-create1k` reopens per kept run: 35–35; Flutter (profile) `s3-create1k` reopens per kept run: 19–19.

**Frust wins S3 on every comparable axis; one reconcile op got slower.** Overall series p50 4.32 vs 19.04 ms at ~113.2 vs ~3.1 fps, and the five Frust reconcile ops all land 340–350 frames per kept series. Against 2026-09-06 four ops moved −6…−9 % (create 10k 19.37, update 14.35, swap 13.27, clear 15.33 ms) but **`create 1k` rose 14.98 → 21.38 ms p50 (+43 %, p95 19.07 → 25.91)** — the one S3 regression in the matrix, on the op that builds 1,000 rows from an empty table, which is where the plan's "no S3 regression" criterion is not met on this device. Flutter's `create 1k` still lands only 2 frames per series (9.80 ms), so the per-op rows stay Frust-only reads.

### S7

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | ~128 ms (128/123/130; first of 4 discarded) | ~402 ms (402/402/403; first of 4 discarded) |
| Framework-reported first-frame span | ~114 ms median (`first_frame_presented`, 108–125 across 12 launches; `adapter_ready` median 100 ms) | ~74 ms median (`first_frame_ms`, 62–85 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | 0 samples | 0 samples |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~82.5 MiB (82.3–83.0, 10 kept snapshots) | ~133.8 MiB (133.5–134.0, 10 kept snapshots) |

**Frust starts ~3.1× faster and idles ~38 % lighter; its framework-reported first frame is longer than in September.** External cold start 128 vs 402 ms (`am start -W` TotalTime; 128 on 2026-09-06), idle TOTAL PSS 82.5 vs 133.8 MiB (114.1 before: −27.6 %), idle CPU 0 samples on both. The Frust first-frame span is 114 ms median (95 before) against Flutter's 74, measured from different entry points as always; inside it, `first_encode_done` → `first_frame_presented` fell 53.5 → 10.5 ms [10–12] with the persisted pipelines, while `adapter_ready` rose 36 → 100 ms (see the criteria block above).

### S8

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (profile) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 98 | 292 | n/a |
| write i64 | 96 | 288 | n/a |
| write f64 | 97 | 307 | n/a |
| write String | 71 | 278 | n/a |
| write Vec\<String\> | 72 | 286 | n/a |
| read (unique key, forces channel) bool | 18 | 4461† | ~0 |
| read (unique key, forces channel) i64 | 19 | 4461† | ~0 |
| read (unique key, forces channel) f64 | 19 | 4461† | ~0 |
| read (unique key, forces channel) String | 19 | 4461† | ~0 |
| read (unique key, forces channel) Vec\<String\> | 19 | 4461† | ~0 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust wins S8 decisively** — writes 71–98 vs 278–307 µs/call (~3.0–3.9×; `String` and `Vec<String>` writes fell from 93 / 92 to 71 / 72 since 2026-09-06), a per-key read 18–19 µs against Flutter's only channel-crossing read, the whole-store `reload()` at 4,461 µs. Zero `s8-errors` on either app.

### vs 2026-09-06 (same device, same scenarios)

Frust, Δ positive = slower / worse:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|---|---|
| S1 | 8.55→8.05 | -5.8% | 9.84→8.87 | -9.8% | 10.61→9.30 | 54.73→15.82 | 0.11%→0.00% | 120.1→120.1 |
| S2 | 9.06→6.37 | -29.6% | 11.31→9.55 | -15.6% | 12.86→12.48 | 53.71→24.44 | 0.12%→0.09% | 120.1→119.9 |
| S4 | 4.77→4.45 | -6.7% | 6.17→6.43 | +4.1% | 8.35→7.48 | 71.41→37.18 | 0.08%→0.05% | 120.2→120.0 |
| S5 | 7.93→6.20 | -21.8% | 8.89→6.94 | -22.0% | 10.51→7.57 | 63.45→15.86 | 0.09%→0.00% | 120.1→120.0 |
| S6 | 8.44→5.01 | -40.6% | 9.16→8.81 | -3.8% | 11.01→10.11 | 58.26→25.23 | 0.14%→0.01% | 120.1→120.1 |
| S3 (overall series) | 5.66→4.32 | -23.6% | 12.26→12.63 | +3.0% | 18.98→19.80 | 63.23→28.42 | 1.91%→2.23% | n/a→n/a |

Frust S3 per-op (reconcile frames):

| Op | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | n frames |
|---|---|---|---|---|---|
| s3-clear | 15.18→15.33 | +0.9% | 17.17→18.01 | +4.9% | 340→340 |
| s3-create10k | 20.62→19.37 | -6.1% | 21.75→20.38 | -6.3% | 346→350 |
| s3-create1k | 14.98→21.38 | +42.8% | 19.07→25.91 | +35.9% | 341→344 |
| s3-swap | 14.54→13.27 | -8.7% | 19.14→16.03 | -16.3% | 348→340 |
| s3-update | 15.57→14.35 | -7.8% | 19.20→16.46 | -14.3% | 350→350 |

Frust S7:

| Metric | old | new | Δ |
|---|---|---|---|
| first-frame span median (ms) | 95 | 114 | +20.0% |
| idle TOTAL PSS mean (MiB) | 114.1 | 82.5 | -27.6% |
| external cold start median (ms) | 128 | 128 | +0.0% |

Flutter control on the same session:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|---|---|
| S1 | 9.50→9.47 | -0.3% | 16.96→17.17 | +1.2% | 19.47→19.43 | 87.02→86.47 | 5.12%→5.23% | 97.8→98.9 |
| S2 | 15.46→15.61 | +1.0% | 24.75→24.96 | +0.8% | 28.25→28.40 | 49.27→83.03 | 39.15%→39.78% | 19.6→19.3 |
| S4 | 4.22→4.25 | +0.5% | 4.63→4.66 | +0.6% | 5.77→5.81 | 20.03→68.78 | 0.00%→0.00% | 119.2→119.2 |
| S5 | 6.43→6.59 | +2.5% | 14.26→14.37 | +0.8% | 16.34→16.61 | 69.64→67.36 | 0.77%→0.96% | 108.6→107.2 |
| S6 | 8.30→8.20 | -1.1% | 9.81→9.79 | -0.2% | 10.31→10.31 | 95.67→86.37 | 0.01%→0.01% | 119.2→119.2 |
| S3 (overall series) | 19.22→19.04 | -0.9% | 33.85→33.73 | -0.4% | 35.66→35.19 | 90.35→40.11 | 65.87%→67.07% | n/a→n/a |

The Flutter control moved −1.1…+2.5 % on p50 across the frame scenarios (its S7 first frame −6 %, PSS −13 % against the Mac-built APK of 2026-09-06), so the session is comparable and the Frust moves are real: S1 −5.8 %, S2 −29.6 %, S4 −6.7 %, S5 −21.8 %, S6 −40.6 %, S3 overall −23.6 % on p50, with S4 p95 +4.1 % and S3 p95 +3.0 % the only frame-table lines that got worse. The S3 `create 1k` op (+42.8 %) and the S7 first-frame span (+20 %) are the two regressions this pass records against September.

### Methodology deviations (this device)

1. **Driver grading failed on the Linux host; every frame/plugin block was re-run once.** `matrix.sh`'s `block_sanity` hands grep a pattern that begins with `-` without `--`, which GNU grep parses as an option, so every block read `kept_lines=0-0` and was graded `DEGENERATE` and retried (BSD grep on the Mac that ran 2026-09-06 accepted it). The data under every attempt is complete (2,900–3,600 raw lines per 30 s run); the published series were staged from the driver's per-attempt trees by hand with the driver's own copy rules and self-checks, taking the first attempt of each block except where noted. The session therefore ran 30 blocks instead of 16, which is what pushed the phone over the thermal ceiling (deviation 3). The harness bug is filed; the harness was not edited for this pass.
2. **frust S1 is the second attempt.** The first frust S1 block — the first launch after a fresh install, with the display just pinned to 120 Hz — produced the activity-create, render-path and atlas lines but no startup line and no frame in any of its 12 runs (p50 0.00, capture dark); the retry 6 min later captured normally (3,604 raw lines per run) and is the published series. Nothing else in the session reproduced it; it is recorded, not explained.
3. **Seven thermal cooldown timeouts** (2026-09-06 had one). The doubled S4/S5/S6 load held the phone at 38.4–40.7 °C from the second frust S5 block to the first flutter S6 block; each of those blocks and the frust S7 block waited the full 120 s and proceeded (`warning: still … after 120s wait`). The published frust S5 attempt opened at 36.4 °C, flutter S5 at 39.6 °C, frust S6 at 40.7 °C and flutter S6 at 38.3 °C — the hot blocks fall on both apps, and the numbers above are read with that in mind.
4. **The driver's install log line carries no md5 on Linux** (`md5 -q` is macOS-only); the APK md5s in the facts are the host's `md5sum` of the installed files.
5. **Flutter APK rebuilt on the Linux host** with the same Flutter 3.47.2 / Dart 3.13.2: 69,869,386 B against 70,462,089 B for the Mac build of 2026-09-06; the control's per-frame numbers are within ±2.5 % of September, so the rebuild is not a measured factor.
6. **Flutter's S2 script runs once per launch** (steady scroll to a quarter, fling to the end, steady scroll back to row 0, then idle), while Frust's loops it — a declared asymmetry in both scenario sources, unchanged since 2026-09-05 (575 Flutter S2 raw frames per 30 s run, as in September). The S2 rows compare per-frame cost on the frames each app painted.
7. **Frust `adapter_ready` 36 → 100 ms** (and `first_frame_presented` 95 → 114) against 2026-09-06 while the post-encode first frame fell 53.5 → 10.5 ms — recorded in the criteria block as the open item.
8. As on prior passes: S7 idle CPU is 0 samples on both sides; Flutter release-mode in-app cross-check not captured; S4 parse wall time not recoverable; S8 burst-during-animation variant not run; the APK packaging asymmetry (arm64-only vs fat) is not a size comparison.

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

**Status:** run 2026-10-17 17:08–19:07 UTC, **12 runs × 30 s per scenario per app (S7: 60 s runs), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** **Frust = PROFILE build of `20e0aa2e`** (`main` after the OnePlus 9 / Pixel 5 pass: the engine, render, GPU, Android-shell, shell-common and widget crates are byte-identical to that pass's `87271063`; `frust-core` carries the readability refactor merged since (PR #83, 49 files) — see deviation 2. Built on this Mac with the checkout's own CLI: `frust build apk --profile --target-platform android-arm64 --define FRUST_TRACE_RAW=1`, raw format v4), **Flutter = profile build** (`flutter build apk --profile`, Flutter 3.47.2, rebuilt on this Mac). Raw series under `raw/xiaomi12/frust_profile/<sN>/` and `raw/xiaomi12/flutter/<sN>/` (run-NN.log + run-NN.pss_before/after.txt, stats.txt; S7 adds cpuinfo.txt/coldstart.txt). One unattended `matrix.sh` session from the Mac, all 16 blocks captured and graded `ok` on attempt 1 — no retries. **This pass supersedes the 2026-09-06 pass on this device** (its section: `git show 20e0aa2e:benchmarks/RESULTS.md`).

- Chipset: Snapdragon 8 Gen 1 (SM8450, board `taro`) / Adreno 730, GLES driver V@0615.95. Model: Xiaomi 12 2201123G (serial redacted).
- OS: LineageOS 23.2 — Android 16, build `BP4A.251205.006`, SDK 36 (unchanged since 2026-09-06); the fingerprint now reports the stock `cupid_global` Android 15 `OS3.0.4.0` build (was `OS3.0.3.0`). Not MIUI/HyperOS.
- Display: 1080×2400 @ ~422 dpi; modes id 0 = 60 Hz, id 1 = 120 Hz — the 60 Hz mode was the active one at session start this time (120 Hz on 2026-09-06). **Both `min_refresh_rate`/`peak_refresh_rate` pinned to 120 for every block** (`--pin-refresh 120`, read-back `peak=120.0 min=120.0`, restored to `Infinity`/`0.0` afterwards, mode id 0 active again at the end); Frust's in-app FPS readout showed 121.5 on the S1 capture. Both budgets (16.67 / 8.33 ms) reported; the 8.33 ms column counts the two-thread cost sum, not vsync misses (the OnePlus 9 note still applies).
- High-refresh opt-in engaged on both apps (§6). Achieved (active frames ÷ 300 s): Frust ~120.6–120.7 fps in S1/S2/S4/S5/S6 and ~113.5 in S3; Flutter ~107.6 (S1), ~111.7 (S5), ~119.8–120.1 (S4/S6); S2/S3 Flutter paint is event-driven (~20.7 / ~3.1 fps).
- Brightness fixed 128/255, auto-brightness off (the phone's own 21/255 restored after); airplane on (read-back 1), Wi-Fi off, Bluetooth off (both on before the session and restored), battery saver off, screen timeout 30 min, `svc power stayon true` during blocks; all restored and read back at 19:07 UTC.
- Charger: USB/AC-connected throughout (`AC powered: true`, `USB powered: true` at both ends of the session, real level 100 % before every block); `dumpsys battery unplug` spoofed on-battery state before every block; the `--min-level 30` guard never waited.
- Thermal: gate ceiling 38 °C / 120 s cooldown before every block. Session start 33.0 °C, peak 38.1 °C (after the flutter S5 block), end 33.2 °C. **One cooldown timeout** — the frust S6 block opened at 38.1 °C after the full 120 s wait (505 s wall against 382–384 s; deviation 1); no other block stalled. 60 s S7 blocks 742 / 743 s wall.
- Toolchain: Rust 1.98.1 (pinned), `wgpu` 30.0.1 (pinned); Flutter 3.47.2 stable / Dart 3.13.2. Frust APK arm64-only, 10,104,611 B (md5 de3b1d91…); Flutter APK fat profile build, 69,869,386 B (md5 faba1212… — the same byte count as the Linux rebuild of the OnePlus 9 pass, a different md5) — packaged differently, not a size comparison. Stale installs removed and both installed fresh by the driver, md5 read back by the driver.
- Visual gate (driver screencap per block, all 16 reviewed on a contact sheet plus band luma): **content PASS on every block, both apps.** S5 shows the decoded image column on both apps (mid-band luma 112 Frust / 114 Flutter, every visible cell fully decoded, Frust's column edge-to-edge); no soft keyboard or other foreign window on any capture (Flutter bottom-band luma 19–47; Frust's 86–158 is its own scenario tab bar, 158 on S5 where the column reaches the band).
- Sanitization: driver staging `RAW_OK` (192 run logs, 384 PSS snapshots, 16 stats.txt, S7 cpuinfo/coldstart), whitelist self-check passed, no address or serial in any staged file.

### S1

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 6.58 | 6.97 | 7.25 | 13.33 | 0 (0.00%) | 32 (0.09%) | 36,199 (~120.7 fps avg) |
| Flutter (profile) | 9.52 | 19.07 | 23.22 | 34.97 | 1,787 (5.53%) | 28,927 (89.59%) | 32,290 (~107.6 fps avg) |

Frust pass p50 (ms): rebuild 0.19, layout 0.01, paint 0.44, encode 0.09, acquire 0.09, submit 5.75; layout_us>0 on 36,199/36,199 frames; GPU (gpu_q=1 on 36,159) total p50/p95 4.58/5.14 ms

**Frust wins S1 on every statistic.** p50 6.58 vs 9.52 ms, p95 6.97 vs 19.07, p99 7.25 vs 23.22, worst 13.33 vs 34.97, and 0.00 % vs 5.53 % of frames over the 60 Hz budget at ~120.7 vs ~107.6 fps (on 2026-09-06 Flutter still held the worst frame). Against September Frust's median fell 7.85 → 6.58 ms and its GPU pass time 5.25 → 4.58 ms — a 13 % GPU gain where the Adreno 660 halved (6.45 → 3.47): S1 is still submit-dominated here (5.75 of the 6.58 ms, with the 4.58 ms of GPU work under it), so the paint-class programs bought this device less than the OnePlus 9. No GPU bar was set for this device; the criteria block reports the figure.

### S2

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 5.23 | 6.37 | 11.01 | 21.79 | 17 (0.05%) | 631 (1.74%) | 36,195 (~120.7 fps avg) |
| Flutter (profile) | 14.08 | 24.12 | 27.17 | 35.46 | 1,786 (28.74%) | 5,335 (85.84%) | 6,215 (~20.7 fps avg) |

Frust pass p50 (ms): rebuild 0.07, layout 1.93, paint 0.28, encode 0.05, acquire 0.09, submit 2.82; layout_us>0 on 36,195/36,195 frames; GPU (gpu_q=1 on 36,155) total p50/p95 2.15/2.21 ms

**Frust wins S2 decisively** — p50 5.23 vs 14.08 ms, p95 6.37 vs 24.12, p99 11.01 vs 27.17, and 0.05 % vs 28.74 % of frames over the 60 Hz budget. Frust holds ~120.7 fps through its looped scroll script while Flutter's script runs once and its paint is event-driven afterwards (~20.7 fps over the block; deviation 5). Frust's own median fell 7.09 → 5.23 ms since September: submit 5.54 → 2.82 with the GPU pass at 2.15 ms (3.07 before), while the per-frame layout rose 1.37 → 1.93 ms and is now the largest CPU pass of the frame.

### S4

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.17 | 5.25 | 5.63 | 20.49 | 8 (0.02%) | 33 (0.09%) | 36,180 (~120.6 fps avg) |
| Flutter (profile) | 4.11 | 5.55 | 5.86 | 12.45 | 0 (0.00%) | 3 (0.01%) | 36,021 (~120.1 fps avg) |

Frust pass p50 (ms): rebuild 0.13, layout 0.01, paint 0.07, encode 0.03, acquire 0.14, submit 3.76; layout_us>0 on 36,177/36,180 frames; GPU (gpu_q=1 on 36,140) total p50/p95 0.46/0.56 ms

**Split on S4, the other way round from September.** Flutter takes the median by 0.06 ms (4.11 vs 4.17); Frust takes the tail (p95 5.25 vs 5.55, p99 5.63 vs 5.86) — both at ~120 fps, both essentially never over the 60 Hz budget (0.02 % vs 0.00 %), Flutter's worst frame the better one (12.45 vs 20.49). On 2026-09-06 Frust held the median (3.43 vs 3.92) and Flutter the tail (5.64 vs 7.89): Frust's median rose 3.43 → 4.17 while its p95 fell 7.89 → 5.25 and its p99 8.95 → 5.63 — the distribution tightened around a higher centre, and all of the median move is in submit (3.01 → 3.76 ms) with the GPU pass itself down 0.71 → 0.46 ms. The same 0.06–0.20 ms deficit to Flutter's S4 median appears on the OnePlus 9; it is a named exception in the criteria block.

### S5

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 5.75 | 6.26 | 6.75 | 12.07 | 0 (0.00%) | 96 (0.27%) | 36,195 (~120.7 fps avg) |
| Flutter (profile) | 5.59 | 10.30 | 12.44 | 45.87 | 16 (0.05%) | 5,302 (15.82%) | 33,524 (~111.7 fps avg) |

Frust pass p50 (ms): rebuild 1.34, layout 0.03, paint 0.12, encode 0.02, acquire 0.09, submit 4.08; layout_us>0 on 36,195/36,195 frames; GPU (gpu_q=1 on 36,155) total p50/p95 4.00/4.06 ms

**Split: Flutter the median by 0.16 ms, Frust the tail and the cadence.** Flutter p50 5.59 vs 5.75 ms (the gap was 0.64 ms on 2026-09-06: 5.93 vs 6.57); Frust p95 6.26 vs 10.30, p99 6.75 vs 12.44, worst 12.07 vs 45.87, 60 Hz misses 0.00 % vs 0.05 %, ~120.7 vs ~111.7 fps. Frust's median fell 6.57 → 5.75 with the GPU pass 5.51 → 4.00 ms; its rebuild stays at 1.34 ms median, the image re-upload traffic of the scrolling column. This build emits no `frust-perf img` counter line (deviation 3), so PROTOCOL §7's content gate is read from the capture: the whole visible column is decoded on both apps. The second named exception on this device.

### S6

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.46 | 8.83 | 9.71 | 14.19 | 0 (0.00%) | 3,243 (8.96%) | 36,199 (~120.7 fps avg) |
| Flutter (profile) | 7.67 | 10.10 | 10.90 | 79.93 | 5 (0.01%) | 10,829 (30.14%) | 35,927 (~119.8 fps avg) |

Frust pass p50 (ms): rebuild 0.13, layout 0.26, paint 0.53, encode 0.15, acquire 0.12, submit 3.31; layout_us>0 on 36,199/36,199 frames; GPU (gpu_q=1 on 36,159) total p50/p95 0.67/0.75 ms

**Frust wins S6 on every statistic** — p50 4.46 vs 7.67 ms, p95 8.83 vs 10.10, p99 9.71 vs 10.90, worst 14.19 vs 79.93, 60 Hz misses 0.00 % vs 0.01 % — where 2026-09-06 left Flutter the worst frame and the p99 (6.38 vs 7.82 on the median then). The resident glyph-run cache took Frust's median down 30 % (6.38 → 4.46) with the GPU pass at 0.67 ms (1.19 before); the cost of this scenario is shaping and upload on the CPU side.

### S3

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | 22.46 / 26.82 / 27.85 / 28.36 | 341 | 11.45 / 11.45 / 11.45 / 11.45 | 1 |
| create 10k | 20.52 / 24.01 / 25.07 / 25.17 | 350 | **not captured** | 0 |
| update every 10th of 10k | 15.22 / 18.34 / 18.98 / 21.54 | 350 | **not captured** | 0 |
| swap | 13.91 / 17.02 / 17.81 / 18.14 | 350 | **not captured** | 0 |
| clear | 16.66 / 19.55 / 20.26 / 20.38 | 340 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.11 | 12.16 | 21.46 | 28.36 | 861 (2.53%) | 2,267 (6.66%) | 34,048 (~113.5 fps avg) |
| Flutter (profile) | 14.63 | 29.68 | 31.90 | 40.88 | 340 (36.84%) | 829 (89.82%) | 923 (~3.1 fps avg) |

Cycle health — Frust (profile) `s3-create1k` reopens per kept run: 35–35; Flutter (profile) `s3-create1k` reopens per kept run: 19–19.

**Frust wins S3 on every comparable axis; every one of its five reconcile ops is slower than in September.** Overall series p50 4.11 vs 14.63 ms at ~113.5 vs ~3.1 fps, and the five Frust ops all land 340–350 frames per kept series (Flutter's `create 1k` lands 1; deviation 6). Against 2026-09-06 the per-op medians moved `create 1k` 12.37 → 22.46 ms (+82 %), `create 10k` 12.93 → 20.52 (+59 %), `clear` 10.34 → 16.66 (+61 %), `update` 10.73 → 15.22 (+42 %), `swap` 10.22 → 13.91 (+36 %); the overall series p95 rose 8.59 → 12.16 and its 60 Hz miss rate 0.49 → 2.53 %, while the overall p50 fell (4.83 → 4.11). The pass split of the op frames (the same `[start, end)` windows `stats.py` publishes) says where: on `clear`, `create 10k`, `update` and `swap` the whole move is the UI half — **rebuild median 7.08 → 13.38, 7.64 → 16.21, 5.67 → 11.12 and 4.89 → 10.04 ms** — with the render half cheaper (submit 2.97–4.77 → 2.52–2.95 ms); on `create 1k` rebuild is flat (1.90 → 2.57) and **submit doubles (9.12 → 18.23 ms)**, the same shape as the OnePlus 9's `create 1k` on `87271063` (submit 17.98). The four-op rebuild move is this device's alone so far — the OnePlus 9 on `87271063` read `clear` 15.18 → 15.33 — and it leaves the Xiaomi's reconcile medians where the OnePlus 9's are (13.3–21.4 ms) instead of 3–8 ms under them as in September. The build differs from `87271063` only in `frust-core` (deviation 2), which is the code that runs rebuild, so whether that refactor or a device-specific effect of the thread policy owns the move cannot be separated from this pass; an A/B of `87271063` against `20e0aa2e` on this device is the open item. This is where the plan's "no S3 regression" criterion is not met here.

### S7

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | ~99 ms (98/100/99; first of 4 discarded) | ~377 ms (351/380/377; first of 4 discarded) |
| Framework-reported first-frame span | ~123 ms median (`first_frame_presented`, 106–128 across 12 launches; `adapter_ready` median 107 ms) | ~64 ms median (`first_frame_ms`, 5–71 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | 0 samples | 0 samples |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~48.5 MiB (47.9–49.0, 10 kept snapshots) | ~83.8 MiB (83.2–84.4, 10 kept snapshots) |

**Frust starts ~3.8× faster and idles ~42 % lighter; its framework-reported first frame is longer than in September.** External cold start 99 vs 377 ms (`am start -W` TotalTime; 107 on 2026-09-06), idle TOTAL PSS 48.5 vs 83.8 MiB (54.9 before: −11.7 %), idle CPU 0 samples on both as the script counts them — Frust never appears in the `cpuinfo` snapshots and Flutter only as four `+0% … 0% user + 0% kernel` lines (2 samples at ~0.3 % in September). The Frust first-frame span is 123 ms median (97 before) against Flutter's 64, measured from different entry points as always; inside it, `first_encode_done` → `first_frame_presented` fell 53.5 → 9.0 ms [8–11] over the 12 launches with the persisted pipelines, while `adapter_ready` rose 35 → 107 ms (min 22 → 91, max 50 → 113) — the third Android device with the same move (criteria block).

### S8

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (profile) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 96 | 279 | n/a |
| write i64 | 77 | 276 | n/a |
| write f64 | 78 | 293 | n/a |
| write String | 75 | 248 | n/a |
| write Vec\<String\> | 75 | 244 | n/a |
| read (unique key, forces channel) bool | 23 | 5130† | ~0 |
| read (unique key, forces channel) i64 | 22 | 5130† | ~0 |
| read (unique key, forces channel) f64 | 21 | 5130† | ~0 |
| read (unique key, forces channel) String | 22 | 5130† | ~0 |
| read (unique key, forces channel) Vec\<String\> | 22 | 5130† | ~0 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust wins S8 decisively** — writes 75–96 vs 244–293 µs/call (~2.9–3.8×; the `bool` write fell 107 → 96 since 2026-09-06, the other four are within ±4 %), a per-key read 21–23 µs (+1 µs on four rows) against Flutter's only channel-crossing read, the whole-store `reload()` at 5,130 µs. Zero `s8-errors` on either app.

### vs 2026-09-06 (same device, same scenarios)

Frust, Δ positive = slower / worse:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|---|---|
| S1 | 7.85→6.58 | -16.2% | 10.18→6.97 | -31.5% | 11.28→7.25 | 63.25→13.33 | 0.11%→0.00% | 120.5→120.7 |
| S2 | 7.09→5.23 | -26.2% | 9.05→6.37 | -29.6% | 9.68→11.01 | 64.85→21.79 | 0.05%→0.05% | 120.7→120.7 |
| S4 | 3.43→4.17 | +21.8% | 7.89→5.25 | -33.4% | 8.95→5.63 | 75.06→20.49 | 0.03%→0.02% | 120.7→120.6 |
| S5 | 6.57→5.75 | -12.5% | 7.16→6.26 | -12.5% | 7.38→6.75 | 66.18→12.07 | 0.03%→0.00% | 120.8→120.7 |
| S6 | 6.38→4.46 | -30.2% | 8.91→8.83 | -0.9% | 10.77→9.71 | 79.65→14.19 | 0.12%→0.00% | 120.6→120.7 |
| S3 (overall series) | 4.83→4.11 | -14.8% | 8.59→12.16 | +41.6% | 15.10→21.46 | 64.30→28.36 | 0.49%→2.53% | n/a→n/a |

Frust S3 per-op (reconcile frames):

| Op | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | n frames |
|---|---|---|---|---|---|
| s3-clear | 10.34→16.66 | +61.1% | 20.08→19.55 | -2.6% | 349→340 |
| s3-create10k | 12.93→20.52 | +58.7% | 23.00→24.01 | +4.4% | 360→350 |
| s3-create1k | 12.37→22.46 | +81.6% | 18.39→26.82 | +45.8% | 352→341 |
| s3-swap | 10.22→13.91 | +36.1% | 16.64→17.02 | +2.2% | 354→350 |
| s3-update | 10.73→15.22 | +41.8% | 18.38→18.34 | -0.2% | 359→350 |

Frust S7:

| Metric | old | new | Δ |
|---|---|---|---|
| first-frame span median (ms) | 97 | 123 | +26.8% |
| idle TOTAL PSS mean (MiB) | 54.9 | 48.5 | -11.7% |
| external cold start median (ms) | 107 | 99 | -7.5% |

Flutter control on the same session:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|---|---|
| S1 | 9.45→9.52 | +0.7% | 19.92→19.07 | -4.3% | 23.80→23.22 | 36.40→34.97 | 5.70%→5.53% | 105.3→107.6 |
| S2 | 14.79→14.08 | -4.8% | 25.41→24.12 | -5.1% | 28.57→27.17 | 41.10→35.46 | 33.93%→28.74% | 19.6→20.7 |
| S4 | 3.92→4.11 | +5.0% | 5.64→5.55 | -1.5% | 5.92→5.86 | 66.49→12.45 | 0.00%→0.00% | 119.9→120.1 |
| S5 | 5.93→5.59 | -5.7% | 10.31→10.30 | -0.1% | 12.33→12.44 | 26.67→45.87 | 0.08%→0.05% | 111.1→111.7 |
| S6 | 7.82→7.67 | -1.9% | 10.05→10.10 | +0.4% | 10.90→10.90 | 14.45→79.93 | 0.00%→0.01% | 119.8→119.8 |
| S3 (overall series) | 14.50→14.63 | +0.9% | 30.12→29.68 | -1.5% | 35.73→31.90 | 39.34→40.88 | 37.55%→36.84% | n/a→n/a |

The Flutter control moved −5.7…+5.0 % on p50 across the frame scenarios (its S7 first frame −11 %, PSS −1.7 % against the Mac-built APK of 2026-09-06), so the session is comparable and the Frust moves are real: S1 −16.2 %, S2 −26.2 %, S5 −12.5 %, S6 −30.2 %, S3 overall −14.8 % on p50, with S4 p50 +21.8 % (against a p95 −33.4 %) and S3 overall p95 +41.6 % the frame-table lines that got worse. The five S3 reconcile ops (+36…+82 %) and the S7 first-frame span (+26.8 %, all of it before `adapter_ready`) are the regressions this pass records against September; the S8 reads moved +1 µs.

### Methodology deviations (this device)

1. **One thermal cooldown timeout** (2026-09-06 had two). The frust S6 block opened at 38.1 °C after the full 120 s wait (`warning: still 38.1C after 120s wait`, 505 s wall) and proceeded; the driver graded it `ok` and it is the published series (flutter S6 opened at 37.2 °C). No other block waited; the session peaked at 38.1 °C after the flutter S5 block and ended at 33.2 °C.
2. **The Frust build is `20e0aa2e`, not the `87271063` of the OnePlus 9 / Pixel 5 pass.** Between the two, `main` took the frust-core readability refactor (PR #83: `app.rs`/`event.rs` split into directory modules, named dispatch steps, a `ChildFold` for child dispatch flags — 49 files) and the iOS-harness captures (PR #84, harness only); `git diff --stat 87271063 20e0aa2e` is empty for `crates/frust-engine`, `frust-render`, `frust-gpu`, `frust-shell-android`, `frust-shell-common` and `frust-widgets`. Every frame-table and S8 number is therefore a same-engine read; the S3 rebuild move is read with this difference in mind (S3 prose).
3. **No `frust-perf img` counter line.** The engine on `87271063` and later emits none (0 lines per S5 run against 3,512 on 2026-09-06; the OnePlus 9 pass's series have none either), so the `skipped`/`evicted`/`resident` figures the September section quoted are not available; the S5 content gate rests on the capture.
4. **Flutter APK rebuilt on this Mac** with the same Flutter 3.47.2 / Dart 3.13.2: 69,869,386 B, the byte count of the Linux rebuild used on the OnePlus 9 / Pixel 5 (md5 differs), against 70,462,089 B for the Mac build of 2026-09-06; the control's per-frame numbers are within ±5.7 % of September, so the rebuild is not a measured factor.
5. **Flutter's S2 script runs once per launch** (steady scroll to a quarter, fling to the end, steady scroll back to row 0, then idle), while Frust's loops it — a declared asymmetry in both scenario sources, unchanged since 2026-09-05. The S2 rows compare per-frame cost on the frames each app painted.
6. **S3 Flutter per-op capture:** 1 frame on `create 1k` and none for the other ops (the known `addTimingsCallback` delivery gap) — no cross-app per-op comparison.
7. **Frust `adapter_ready` 35 → 107 ms** (and `first_frame_presented` 97 → 123) against 2026-09-06 while the post-encode first frame fell 53.5 → 9.0 ms — recorded in the criteria block as the open item, now on all three Android devices.
8. As on prior passes: S7 idle CPU is 0 samples on both sides; Flutter release-mode in-app cross-check not captured; S4 parse wall time not recoverable; S8 burst-during-animation variant not run; the APK packaging asymmetry (arm64-only vs fat) is not a size comparison.

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
| on-disk arm64 `.so` (stripped), 2026-09-19 | 8.74 MiB (9,167,552 B) | 6.77 MiB (7,103,296 B) | n/a |
| iOS release `.app` (`du -sk`), 2026-09-19 | 11.30 MiB (11,568 KB) | not built | 16.64 MiB |
| iOS `Runner` binary (unstripped), 2026-09-19 | 11.21 MiB (11,755,184 B) | not built | n/a |
| iOS `Runner` binary (**stripped**, plain `xcrun strip` scratch copy), 2026-09-19 | **8.16 MiB** (8,559,536 B; real `xcodebuild archive` Runner measures 8,534,544 B, −0.29 %) | not built | n/a |

The on-disk `.so` row was re-measured on 2026-09-19, after `jni`, then `tokio` and the three
accesskit crates, and then `wgpu-core` and `wgpu-types` left the cold set (per-lever table
below); the other Android rows were not, and predate all three changes. Against it, the arm64
split-APK and in-APK `.so` rows read low by at least those three per-lever deltas (`jni`
+30,184 B lean / +30,576 B db-on; `tokio` + accesskit +21,944 B lean / +39,384 B db-on;
`wgpu-core` + `wgpu-types` +237,104 B lean / +254,904 B db-on, each measured on its own base).
The universal APK carries one `.so` per ABI, so it likely reads low by more than that — not
measured. The in-APK and on-disk `.so` rows are also not directly comparable: beyond those three
changes, their lean gap includes ~4.3 KB of unrelated work that landed between the measurements.

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
| Per-crate `opt-level = "z"` cold set (17 crates, `wgpu-hal` excluded) | 7,523,288 → 6,833,696 | −689,592 | `.text` −17.9 %; unwind tables measured-keep (−645,376 B rejected — removes native backtraces); **the 5 % render-CPU bar was adjudicated on the OnePlus 9 on 2026-09-17 and cleared** — see the dated note below. **iOS-relevant**: `[profile.release.package.*]` is not target-scoped, so the cold set applies to `aarch64-apple-ios` as well — with a second-order cost there, see the iOS-delta note below. `jni`, `tokio`, the three accesskit crates, `wgpu-core` and `wgpu-types` have since left this set (rows below) |
| `jni` removed from the cold set, 2026-09-18 | 6,812,880 → 6,843,064 | +30,184 | db-on +30,576 B (8,841,504 → 8,872,080). The render-CPU bar could not see `jni`'s per-frame work: Android's per-frame JNI polls run in separate JNI entries after `nativeOnFrame` returns, outside every span `total_us` sums. Removed rather than kept on an unmeasured exception. The bar does see per-frame cost inside the frame's own spans — the submit-path cost S5 shows below comes from the cold set |
| `tokio` and the three accesskit crates removed from the cold set, 2026-09-19 | 6,843,736 → 6,865,680 | +21,944 | db-on +39,384 B (8,872,752 → 8,912,136). The render-CPU bar could not see either one's per-frame work (the reactive pump; the semantics publish on every frame whose semantics changed), so both were measured directly instead — see the microbench note below: at `z`, accesskit's publish failed the cold set's CPU rule on a little core (at a 2,000-node tree), and `z` saved `tokio` no size. Removed on the owner's decision after that pass |
| `wgpu-core` and `wgpu-types` removed together from the cold set, 2026-09-19 | 6,866,192 → 7,103,296 | +237,104 | db-on +254,904 B (8,912,648 → 9,167,552). Neither is cold: `wgpu-core` sits on the per-frame queue-write/submit path (the dated S5 attribution note below isolated ~0.2 ms of `submit` p50 to its `z` build) and `wgpu-types` was larger at `z`, not smaller. That note measured each alone (`wgpu-core` +246,232 lean / +263,104 db-on; `wgpu-types` −6,888 lean / −7,264 db-on); removing them together was not measured there, hence this row. This pass's own full-set build reads 512 B above the previously committed on-disk row (6,865,680 / 8,912,136) — the same build-to-build offset the S5 attribution note recorded and did not decompose, so it cancels in this row's own Δ. Removed on the owner's decision after that pass |
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
per-crate isolation was run in this pass (`wgpu-core` was the plausible candidate; the dated S5
attribution note below isolated it on 2026-09-19 and confirms `wgpu-core`). It passes, but the
headroom is not wide: S5's +3.4 % leaves 1.6
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

**S5's `submit` cost attributed to `wgpu-core` — OnePlus 9, 2026-09-19.** The cold-set bar note
above left one real signal unattributed: both cold-set S5 blocks put ~0.2 ms more into `submit`.
This pass re-confirms that cost against the current 12-crate cold set, then removes one override
at a time. **The cost comes from `wgpu-core`'s `opt-level = "z"`.** Removing that one block takes
209.0 / 193.5 µs off S5's `submit` p50 in the two rounds, which is 94 % / 87 % of the whole cold
set's gap. Removing `gpu-allocator`, `ash` or `wgpu-types` moves it by no more than the
same-binary drift.

*Method.* The pass used `--profile` arm64 APKs of `benchmarks/frust_bench`, built with
`frust build apk --profile --define FRUST_TRACE_RAW=1 --target-platform android-arm64`. The APKs
differ only in which `[profile.release.package.*]` blocks `benchmarks/frust_bench/Cargo.toml`
carried. Each variant was a temporary edit, reverted after its build. A second full-set build
reproduced the first APK byte for byte (same md5), so each variant differs from the full set by
its override alone. Every block drove S5 only, through one `run.sh` call: 12 runs × 30 s, the
first 2 discarded, leaving 35,980–36,011 kept frames. Each block ran behind `device_state.sh`'s
fixed brightness and 38 °C cooldown gate. Every block reinstalled its APK and checked the
installed copy's md5 on the device. The per-block tables and a manifest of the raw per-run logs
are attached to the task that produced this pass.

The rig was the OnePlus 9 on USB adb, with airplane mode on and `min_refresh_rate` and
`peak_refresh_rate` pinned to 120. `dumpsys battery unplug` was issued before each block. As
before, that is a framework-level override only: the phone stayed on USB power and read 100 % at
every block boundary, so these numbers are not battery-run numbers. All 34 blocks held
119.9–120.0 fps, and battery temperature ran 27.1–38.1 °C. In six step-2 blocks the cooldown
gate timed out at 38.1 °C and the block proceeded: three full-set blocks and one each of the
`ash`, `wgpu-types` and `gpu-allocator` variants, none of them a `wgpu-core` variant block.

*Arithmetic.* Each per-block figure is `stats.py`'s nearest-rank percentile over the pooled kept
frames. A quad's Δ is the mean of its two variant blocks minus the mean of its two full-set
blocks. Its A/A drift is the gap between the two blocks of one binary inside it. **The A/A floor
is the largest such gap on `submit` p50 in the session, 61 µs.** The 20 within-quad pairs span
0–61 µs, and the two back-to-back full-set pairs at round boundaries are 25 and 32 µs. A removal
counts as responsible only if its Δ exceeds that floor in both rounds.

*Step 1: the cost is still there.* Full set (Z) vs no overrides (N), order Z,N,N,Z × 2. Δ is
N − Z, the effect of removing the set.

| Round | `submit` p50, Z → N (ms) | Δ `submit` p50 | Δ `submit` p95 | Δ cost-sum p50 | A/A `submit` p50 (Z / N pair) |
|---|---|---|---|---|---|
| 1 | 5.870, 5.813 → 5.600, 5.601 | **−241.0 µs** | −158.5 µs | −160.0 µs | 57 / 1 µs |
| 2 | 5.788, 5.785 → 5.577, 5.593 | **−201.5 µs** | −140.5 µs | −426.5 µs | 3 / 16 µs |

The cold set costs S5 +4.30 % / +3.61 % on `submit` p50 (Z over N). That is the same size as on
2026-09-17 (5.58/5.59 → 5.78/5.80 ms). Every Z block (5.785–5.870 ms) sits above every N block
(5.577–5.601 ms). The CPU-work phases and GPU time do not move: rebuild+layout+paint+encode
differs by 6.0 / 6.5 µs at p50, and `gpu_main` p50 by 3.5 / 4.5 µs.

Cost-sum p50 reads +2.04 % / **+5.54 %** (Z over N), a mean of +3.8 %. The second round crosses
the 5 % bar. `acquire` accounts for 149 µs of that round's 426.5 µs; the Z pair's `acquire` gap
alone is 90 µs, and within-quad `acquire` p50 gaps reach 224 µs across the session. Cost-sum p50
gaps reach 294 µs. Cost-sum therefore cannot resolve a 0.2 ms effect one quad at a time;
`submit`, whose floor is 61 µs, can.

S5 was the one scenario in which the 2026-09-17 bar found this signal, and its workload explains
why. It streams 240 distinct 256×256 images through the mobile-tier atlas
(`budget=1024x1024x4`), which holds about 60 of them. So the engine evicts and re-uploads about
1.5 images every frame: `frust-perf img … evicted=` sums to 1.53 per kept frame in a Z block and
1.54 in an N block. Each re-upload is one `queue.write_texture`, added to the frame's own queue
writes and submits. All of it runs inside `submit_us`, and all of it goes through `wgpu-core`.

*Step 2: isolation.* Each variant removes one override from the full set. Each variant ran as
Z,V,V,Z, with neighbouring quads sharing a Z block. Round 1 took the variants in the order
`wgpu-core`, `gpu-allocator`, `ash`, `wgpu-types`, and round 2 reversed it. `image` and `png`
were not candidates. S5 decodes each of its 240 images once per process, one cached decode task
per image id, not once per frame. Its per-frame image work is the atlas re-upload, and that path
does not call either crate. Δ is V − Z.

| Override removed | Round | `submit` p50, Z → V (ms) | Δ `submit` p50 | A/A (Z / V pair) | Δ cost-sum p50 |
|---|---|---|---|---|---|
| `wgpu-core` | 1 | 5.836, 5.812 → 5.614, 5.616 | **−209.0 µs** | 24 / 2 µs | −255.0 µs |
| `wgpu-core` | 2 | 5.762, 5.781 → 5.589, 5.567 | **−193.5 µs** | 19 / 22 µs | −325.5 µs |
| `gpu-allocator` | 1 | 5.812, 5.812 → 5.790, 5.835 | +0.5 µs | 0 / 45 µs | −71.0 µs |
| `gpu-allocator` | 2 | 5.802, 5.762 → 5.785, 5.827 | +24.0 µs | 40 / 42 µs | −133.5 µs |
| `ash` | 1 | 5.812, 5.806 → 5.849, 5.820 | +25.5 µs | 6 / 29 µs | −193.5 µs |
| `ash` | 2 | 5.805, 5.802 → 5.792, 5.755 | −30.0 µs | 3 / 37 µs | +31.5 µs |
| `wgpu-types` | 1 | 5.806, 5.818 → 5.824, 5.763 | −18.5 µs | 12 / 61 µs | +122.0 µs |
| `wgpu-types` | 2 | 5.850, 5.805 → 5.814, 5.824 | −8.5 µs | 45 / 10 µs | +10.0 µs |

**Attribution: `wgpu-core`.** Removing it exceeds the 61 µs floor in both rounds, by 3.4× and
3.2×. It closes 94 % and 87 % of step 1's mean 221 µs gap. Its four variant blocks (5.567–5.616
ms) are indistinguishable from the four no-cold-set blocks (5.577–5.601 ms). So the other eleven
overrides combined leave no residual that can be resolved across the session; that is a
cross-step comparison, not an ABBA. `gpu-allocator`, `ash` and `wgpu-types` stay inside the
floor in both rounds and show no consistent sign (`ash` +25.5 µs then −30.0 µs). Where inside
`wgpu-core` the time goes was not isolated. The cost-sum column is noisier than `submit` in every
quad, for the `acquire` reason above. Read it as not contradicting the attribution, not as a
second measurement of it. This was not measured on iOS, where the same override applies because
`[profile.release.package.*]` is not target-scoped.

*Size give-back.* Each size is the stripped arm64 release `libfrustbench.so`, measured with
`wc -c`. The builds are `cargo ndk -t arm64-v8a build --release`, with `--no-default-features
--features lean` for the lean build, run from `benchmarks/frust_bench`. Each removed one override
from the full set as a temporary edit, reverted after it.

| Override removed | lean (B) | Δ lean (B) | `db` on (B) | Δ `db` on (B) |
|---|---|---|---|---|
| none (full set) | 6,866,192 | — | 8,912,648 | — |
| `wgpu-core` | 7,112,424 | **+246,232** | 9,175,752 | **+263,104** |
| `wgpu-types` | 6,859,304 | −6,888 | 8,905,384 | −7,264 |
| `gpu-allocator` | 6,870,168 | +3,976 | 8,916,776 | +4,128 |
| `ash` | 6,877,624 | +11,432 | 8,924,216 | +11,568 |
| all twelve | 7,503,560 | +637,368 | 9,576,784 | +664,136 |

Two full-set builds, one before the variants and one after, were byte-identical. Both come out
512 B above the committed on-disk row (6,865,680 / 8,912,136 B). That offset is common to every
build here and was not decomposed, so it cancels in each Δ. `wgpu-core`'s give-back is 38.6 % of
the whole cold set's lean saving and 39.6 % of its `db`-on saving. Each removal was measured
alone; removing `wgpu-core` and `wgpu-types` together was not measured.

**Finding and recommendation (the owner decides; no manifest was changed).** `wgpu-core` is not
cold. Every frame's queue writes and submits run through it. On S5's upload-heavy frames its `z`
build costs about 0.2 ms of `submit`, 3.35–3.59 % of the full-set p50, and that is nearly all of
the cold set's S5 cost. The manifests' rationale comment groups it with crates that "run at
pipeline-creation and resource-allocation time, not per frame". That description is wrong for
`wgpu-core` whichever way this is decided. Its override buys 246,232 B lean and 263,104 B
`db`-on, so this is a real trade, not a free fix.

**Recommended: take `wgpu-core` out of the cold set.** Its cost sits on the per-frame upload and
submit path. S5 is the only scenario measured with per-frame uploads, so how large the cost gets
under heavier upload traffic is unknown. In one of this pass's two step-1 rounds, the cold set's
S5 cost-sum p50 already reached +5.54 % against the 5 % bar. The same manifests keep `wgpu-hal`,
which is also on the submit path, out of the set: its gain was 37 KB and its cost stayed inside
the noise. `wgpu-core`'s gain is larger, but its cost is resolved. Keeping it is defensible only
as a named exception, with the comment corrected.

Separately, **`wgpu-types` can leave at no cost**. At `z` it is larger (removing its block saves
6,888 B lean / 7,264 B `db`-on), and it moved S5 by nothing resolvable. `gpu-allocator` and `ash`
save 3,976 and 11,432 B lean with no resolvable S5 cost, so nothing here argues for moving them.

**Applied on 2026-09-19:** on the owner's decision, both `wgpu-core` and `wgpu-types` left the
cold set (per-lever table above, combined-removal row), and the manifests' rationale comment was
corrected to match.

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
`png` +47 — members of the cold set at the time (`tokio`, `wgpu_core` and `wgpu_types` have
since left it) whose functions
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

### Template baseline — `flutter create` vs `frust create` counter, 2026-10-05

The benchmark apps above carry the whole scenario suite; this one-off measures the floor
instead: each framework's freshly generated counter app, unmodified, built for release. Both
are a Material counter (app bar, centred count, increment FAB). Flutter 3.47.2 / Dart 3.13.2
(`flutter create`, then `flutter build apk --release [--split-per-abi]` and `flutter build ipa
--release`); frust 0.5.2 from crates.io (`frust create`, then `frust build apk --release
[--split-per-abi]` and `frust build ipa --release --export-method app-store-connect`); Rust
1.98.1, Xcode 27.0 (27A266a). Both IPAs are App Store exports signed with the same
development team. The Frust APKs are signed with a throwaway keystore, because `frust build`
refuses a debug-signed release; Flutter's template signs release with the debug key. Signing
does not change the size class. Frust's universal APK figure comes from the build log,
because the later `--split-per-abi` build replaced the file.

| Axis | Frust | Flutter |
|---|---|---|
| Android universal release APK (3 ABIs) | **18.46 MiB** (19,354,835 B) | 42.06 MiB (44,103,448 B) |
| Android arm64-v8a split APK | **6.47 MiB** (6,783,650 B) | 14.77 MiB (15,487,482 B) |
| Android armeabi-v7a split APK | **4.87 MiB** (5,108,300 B) | 12.11 MiB (12,699,582 B) |
| Android x86_64 split APK | **7.36 MiB** (7,722,039 B) | 16.14 MiB (16,922,108 B) |
| in-APK arm64 native code | 6,639,704 B (`libfrust_counter.so`) | 14,960,032 B (`libflutter.so` 11,747,864 + `libapp.so` 3,212,168) |
| iOS `.ipa` as exported | **3.78 MiB** (3,966,346 B) | 18.29 MiB (19,180,376 B) |
| iOS `Payload/` only, re-zipped (`zip -9`) | **2.87 MiB** (3,004,371 B) | 5.90 MiB (6,191,031 B) |
| iOS installed `.app` (`du -sk`) | **6.22 MiB** (6,368 KB) | 13.90 MiB (14,236 KB) |
| iOS native code | 6,433,392 B (`Runner`) | 12,881,744 B (`Flutter.framework` 9,414,288 + `App.framework` 3,467,456) |

The exported `.ipa` row overstates the gap. Both exports include a `Symbols/` directory for
App Store crash symbolication, which is not delivered to devices: 4,564 KB for Frust and
58,024 KB for Flutter. The `Payload/`-only and installed `.app` rows are the like-for-like
figures. On Android and in the iOS `Payload/`, the Frust template is about 2.1–2.5× smaller.
Flutter's floor is its engine (`libflutter.so` / `Flutter.framework`), plus 0.86 MB of ICU
data and the bundled Cupertino icon font on iOS. Frust ships a single native binary.

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
