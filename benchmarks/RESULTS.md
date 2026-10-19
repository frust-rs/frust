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
device's delta table ("vs 2026-09-06" on all four devices) is `harness/compare.py` over two of those JSON
dumps, both regenerated from the committed raw trees; Δ positive = slower. Prose mentions of an
earlier pass's own idle-memory figure are likewise recomputed from that pass's raw series in git
history, not copied from its published text (`de265855` Pixel 5, `32597904` OnePlus 9, `20e0aa2e`
Xiaomi 12 and iPhone SE).

> **Frust-fast close-out pass, 2026-10-17 (all four devices).** Every Frust series in the OnePlus 9
> and Pixel 5 sections is a `--profile` build of `87271063` — `main` after the engine fast paths
> (paint-class programs, glyph quads, the resident glyph-run cache, the parallel strip walk, the
> staging ring and bind-group cache), the Android thread policy and gated JNI polls, and the
> first-frame warm-up and pipeline-cache persistence — against a Flutter 3.47.2 profile build; the
> Xiaomi 12 and iPhone SE sections are `--profile` builds of `20e0aa2e`, the same engine, render, GPU,
> shell and widget crates with `frust-core`'s readability refactor (PR #83) on top (their deviation 2).
> It replaces every device's 2026-09-06 series under `benchmarks/raw/` (PROTOCOL §2.6; the 2026-09-06
> sections are in git history, `git show 87271063:benchmarks/RESULTS.md` for the two phones, `git show
> 20e0aa2e:benchmarks/RESULTS.md` for the Xiaomi 12 and iPhone SE) and each section carries a delta
> table against that pass for both apps. Each device ran the full 16 blocks in one unattended
> `matrix.sh` session: OnePlus 9 (USB, 120 Hz pinned, 07:24–11:04 UTC) and Pixel 5 (adb-over-Wi-Fi,
> unpinned, on battery, 07:24–10:54 UTC) driven from a Linux host for the first time; iPhone SE (USB
> `devicectl`, 60 Hz, 17:05–19:22 UTC, Xcode 27.0 open for Instruments) and Xiaomi 12 (USB, 120 Hz
> pinned, 17:08–19:07 UTC) from the Mac, concurrently. The iPhone SE section carries this pass's first
> iOS screenshot, thermal, external cold-start and idle captures (harness PR #84).
> Toolchain for this pass: Rust 1.98.1 (pinned), Flutter 3.47.2 stable / Dart 3.13.2 (profile apps
> rebuilt on the Linux host for the two phones, on the Mac for the Xiaomi 12 and iPhone SE), Xcode 27.0
> (27A266a), `wgpu` 30.0.1 (pinned).

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

## Success criteria — frust-fast close-out pass, 2026-10-17 (all four devices)

The frust-fast plan's criteria on all four devices, each with the number from this pass. Misses are recorded as misses.

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
| iPhone SE S4 work row p50 ≤ 2.0 ms (from 3.69; Flutter 1.38) | **2.37** (Flutter 1.34) | **MISS by 0.37 ms** |
| iPhone SE S5 work row p50 ≤ 3.0 ms (from 5.47; Flutter 2.62) | **5.50** (Flutter 2.73 — a placeholder-dominated column, its S5 gate) | **MISS by 2.50 ms** |
| iPhone SE S6 work row p50 ≤ 4.3 ms (from 5.57; Flutter 4.30) | **3.87** (Flutter 4.33) | **MET** |
| iPhone SE S2 work row p50 ≤ 5.0 ms (from 6.47; Flutter 4.93) | **5.96** (Flutter 4.93) | **MISS by 0.96 ms** |
| iPhone SE `first_frame_presented` median ≤ 85 ms (from 100, bimodal on the font preinit) | **201 ms** (95–304; no launch under 95); `adapter_ready` 22 → 84 ms, `font_preinit_joined` four launches at 56–113 / eight at 177–273 (was six 51–65 / six 100–159); first frame = join + 5–33 ms in 11 of 12 | **MISS on every launch**, worse than September |
| iPhone SE engine encode (S1 / S6 `frust-perf enc` windows, p50 of window p50s): per cached glyph ≤ 1.0 µs (from 4.54 / 2.69), S6 total ≤ 1.5 ms (from 3.99), S1 total ≤ 1.5 ms (from 4.71), `upload` ≤ 0.2 ms, `replay` ≤ 0.1 ms | per glyph **1.005** µs (S6) / 0.642 (S1); S6 total **1.565 ms**; S1 total **4.037 ms** (3.16 of it the strip walk); upload 0.041 / 0.089 ms; replay 0.0004 / 0.0003 ms | upload, replay and the S1 per-glyph **MET**; S6 per-glyph **MISS by 0.005 µs**; S6 total **MISS by 0.065 ms**; S1 total **MISS by 2.54 ms** |
| iPhone SE no regression: S3 and S8 rows, idle | S3 overall p50 −22 %, ops −5…−34 % except `clear` +1.4 % (rebuild +44…+48 % inside four ops, under the acquire wait); **S8 writes 3–5 → 18–28 µs, reads 1 → 3–6 µs** (iOS only); idle rows first captured (1.8 % CPU, 38.9 MiB) | **MISS on S8** |
| iPhone SE final matrix: Frust at or below Flutter's p50 (work row) on every row, or the exceptions named | Frust ahead on S6 (3.87 vs 4.33), S8 and the S3 series; behind on **S1 (5.57 vs 5.05)**, **S2 (5.96 vs 4.93)** and **S4 (2.37 vs 1.34)**; S5 not comparable (Flutter's column is placeholders); external cold start **183 vs 171 ms** (first capture) | **NOT MET** — three rows and the cold start behind |

**The first frame after encode is fixed; the span before it moved the other way.** `first_encode_done` → `first_frame_presented`
fell from 53.5 to 10.5 ms (OnePlus 9) and from 123.5 to 13.0 ms (Pixel 5) — the pipeline-cache blob now holds the engine
pipelines (`PERFORMANCE_BASELINES.md` § Engine close-out per device: 117 KB, 18 pipelines, against 328 B and none) and the first
frame steals only the variants it draws. But `adapter_ready` (the stamp after adapter, device and renderer creation, which also
covers seeding the driver cache from that blob and queueing the warm-up) rose from 36 to 100 ms on the OnePlus 9 and from 44 to 180 ms
on the Pixel 5, so the whole `first_frame_presented` span is **longer** than on 2026-09-06 (95→114 ms and 172→202 ms) while
`am start -W` is flat (128→128) or better (159→141). The two stamps bound different work; which of the intervening changes
(driver-cache seeding, the hardware-buffer device probe, the warm-up worker starting beside device creation) owns the extra
device-open time was not measured in this pass and was handed back; the 2026-10-19 split owns it (Follow-ups, below). The Xiaomi 12 repeats the
pattern from the Mac: 53.5 → 9.0 ms after encode, `adapter_ready` 35 → 107 ms, `first_frame_presented` 97 → 123,
`am start -W` 107 → 99 — three devices, one direction.

**Xiaomi 12: every S3 reconcile op got slower, and on four of the five it is rebuild — the iPhone SE shows the same four-op move.** `create 1k` 12.37 → 22.46 ms p50,
`create 10k` 12.93 → 20.52, `clear` 10.34 → 16.66, `update` 10.73 → 15.22, `swap` 10.22 → 13.91. On the last four the
whole move is the UI half — the op frames' rebuild median roughly doubled (4.89–7.64 → 10.04–16.21 ms) while submit got
cheaper; on `create 1k` it is submit (9.12 → 18.23 ms), the shape the OnePlus 9 shows. The Xiaomi ran `20e0aa2e`, which
differs from the two phones' `87271063` only in `frust-core` (the readability refactor, PR #83); the iPhone SE on the same
build moved the same four ops' rebuild +44…+48 % under its acquire wait, and the OnePlus 9 on `87271063` shows no rebuild
move (`clear` 15.18 → 15.33). **The A/B run after the pass clears the refactor** (2026-10-18, Xiaomi 12 S3 subsection):
`87271063` and `20e0aa2e` measure alike, the September tree rebuilt today measures 16.1–16.6 ms `create 10k` rebuild on
the phone's current firmware, pinning the UI thread to the gold cores returns `20e0aa2e` to 7.96 ms and the platform's
interaction boost (input events flowing, as in real use) takes it to 5.18 — the reconcile cost is unchanged; the move is the walt scheduler keeping the UI thread on the 1.79 GHz silver cores through the bursts.
The iPhone's +44…+48 % is a different class: the September tree rebuilt and run on today's iPhone SE
(iOS 26.7.1, 2026-10-19) keeps the September rebuild medians while `main` the same day reproduces the 2026-10-17 ones, so on the
A13 the move rides the build (bisect range `c705922b..57c08eb1`, the `main` merges 2026-10-03 → 10-14) — see the iPhone SE
section's September-tree block.

**iPhone SE: the first frame doubled, on both of the things it waits for.** `adapter_ready` 22 → 84 ms is the Android
`adapter_ready` move on a fourth device — iOS carries no pipeline-cache blob and no hardware-buffer probe, which leaves the
first-frame warm-up starting beside device creation as the one candidate common to all four (a candidate, not a
measurement) — and the font preinit join is slower too (eight of twelve launches at 177–273 ms against six at 100–159 in
September), so `first_frame_presented` lands at 201 ms median (95–304) where 2026-09-06 read 100. The frame still presents
5–33 ms after the join in eleven launches: closing this remains September's font-preinit question, now with a slower device
open under it. The follow-ups plan's f2-01 and f2-02 took this up (Follow-ups, below); its p5-02 is superseded by f2-02.

**iPhone SE: the S8 plugin rows are 3–6× September's.** Writes 3–5 → 18–28 µs/call and reads 1 → 3–6 on the same preferences store, while the same build's Android rows are flat and Flutter's iOS rows moved −13…+16 %. A three-run A/B after the matrix with Xcode quit reads the same as with it open (writes 15–29 µs), so the Instruments rig is not the cause. **Separated 2026-10-19: the build.** The September tree (`36ef20db`) rebuilt and run on today's phone (iOS 26.7.1) reads writes 3–6 µs / reads 1 µs, while `main` the same day reads 19–28 / 4–6 — the OS is exonerated; the regression lies in the `c705922b..57c08eb1` range (iPhone SE section, September-tree block).

**Follow-ups (2026-10-20): `adapter_ready` — shader-module creation owned the move, and f2-01 fixed it.** The sub-step stamps (PR #97) put `shader_modules_ms` at 92 % of the install gap on the OnePlus 9 (76 of 82 ms fresh, 75 of 83 warm), 90 % on the Pixel 5 (150 of 167 fresh, 151 of 168 warm) and 84 % on the iPhone SE (69 of 82) — twelve `create_shader_module` calls where September created four; the driver-cache seed, the hardware-buffer probe and the warm-up spawn cost 0 (each device's `### Startup sub-step split (2026-10-19)` block). f2-01 (shader modules created on first use, the first frame's four on a pool started at install; PR #102) brings `adapter_ready` back to September's figures on all three measured phones: OnePlus 9 111 → **32** ms (September 36), Pixel 5 189 → **40** (44), iPhone SE 82 → **12** (22); `first_frame_presented` 124 → 67, 214 → 108 and 169 → 108.5 ms (`### Device-open fix re-measure`, Android 2026-10-19 and iPhone 2026-10-20). It shipped with the Android post-encode guard missed: `first_encode_done → first_frame_presented` 11 → 32 ms on the OnePlus 9 and 14 → 54 on the Pixel 5 against ≤ 25, the iPhone 6 → 8.5 (inside it). f2-03's pre-init lever for that residue is a **NO-GO** (OnePlus 9 28 ms [23–33], the modules cannot exist before the pre-init thread owns the device; `### Pre-init shader modules re-measure (2026-10-20) — NO-GO`; `android-first-frame-carries-shader-module-creation` in `docs/LIMITATIONS.md`). The Xiaomi 12 was not attached for this plan and was not re-measured; its `adapter_ready` 107 ms above stands as measured.

**Follow-ups (2026-10-20): iPhone SE S8 plugin rows — the build, not iOS.** The September tree (`36ef20db`) rebuilt and run on iOS 26.7.1 reads writes 3–6 µs / reads 1 µs; `main` the same day reads 19–28 / 4–6 (`### September tree rebuilt on iOS 26.7.1 (2026-10-19)`). Carried open as `act_000001a14dde1c5b16CCVZiT`, bisect range `c705922b..57c08eb1`.

**Follow-ups (2026-10-20): iPhone SE S3 rebuild — the build, same block.** September tree rebuild p50 4.27 / 4.24 (`clear`), 9.26 / 9.15 (`create 10k`), 4.00 / 3.89 (`update`), 2.98 / 3.00 (`swap`) ms against `main` 5.98 / 6.02, 12.94 / 12.89, 5.54 / 5.58, 4.38 / 4.42; `create 1k` flat (1.7). The same range, carried open as `act_000001a14dde1e08KL4q0yxt`.

**Follow-ups (2026-10-20): OnePlus 9 S3 `create 1k` — split, not fixed.** The 14.98 → 21.38 ms p50 is the op frame's own `submit` (12.56 → 17.97 ms) plus the frame after it (7.63 → 11.27 ms); frames 10–20 of the cycle are cheaper than September's, so the whole cycle's submit falls 114.3 → 92.4 ms. It is not GPU work, not the parallel strip walk (`NO_PARALLEL` op frame 18.18 ms) and not the resident-run cache (17.80 ms) (`### create 1k split (2026-10-19)`, PR #99); the settle-window candidate is untested and `act_000001a14dde1e09f8S3m5Hm` stays open.

**Follow-ups (2026-10-20): iPhone SE first frame — bound to the Core Text font join; deferring it is a NO-GO.** After f2-01 the median is 108.5 ms [75–377] on `frust_bench` with `adapter_ready` 12; every launch's first rebuild follows `font_preinit_joined` (94.5 ms [52–322]) by 8–40 ms. f2-02 deferred the system fonts: huddle's first frame fell 112.5 → 42 ms, but the first screen draws in another face until adoption at 203.5 ms [84–319] and swaps with a re-layout, and one cached layout stays in the old face (`### Deferred system fonts (f2-02) — NO-GO (2026-10-20)`; code kept unmerged on `perf/ios-deferred-system-fonts` `19adf5eb`; `shell-ios-first-frame-above-goal` in `docs/LIMITATIONS.md`). `act_000001a14dde1e0aHYkRX5Qx` is resolved; p5-02 (`tsk_000001a134bec1e04X7VyAND`, still `ready` on the completed frust-fast board) is superseded by f2-02.

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

### Startup sub-step split (2026-10-19)

**Method.** The OnePlus 9's recipe on this phone (its block states the build, the script and the key semantics; same APK, md5 `c4ae3d9a…`), with this device's own protocol: **adb-over-Wi-Fi through a tunnel, radios left as found** (airplane off, Wi-Fi up carrying the link, Bluetooth on — the §3 radios-off state not applied, as on 2026-10-17's pass on this device, deviation 1), **refresh not pinned**, on battery (79 → 75 %, `dumpsys battery unplug` before each leg), brightness 128 during the runs (restored to 32), `svc power stayon` during the runs, 30 min timeout, same ROM as 2026-10-17 (`UP1A.231105.001.B2`, incremental `11260668`, serial redacted), no sampler; `device_state.sh` gate, 33.5 → 33.6 °C and 33.4 → 33.8 °C. Two series of `run.sh s7 --runs 12 --duration 60`: fresh install, then — after one 20 s rendering S1 launch — the warm series, for the reason the OnePlus 9 block gives (the fresh-install series restores no pipeline-cache blob). All 12 launches of every series carry a startup line; medians and [min–max] over all 12.

| Key (ms) | 2026-10-17 (`87271063`, matrix S7 block, blob present) | today, fresh install (no blob on any launch) | today, after one rendering launch (blob restored) |
|---|---|---|---|
| `preinit_joined` | 20 [11–26] | 17 [7–91] | 20 [3–29] |
| `cache_loaded` | 22 [11–26] | 18 [7–92] | 21 [3–30] |
| `pipeline_cache_restored` | 22 [11–26] | **absent on all 12** | 21 [3–30] |
| `adapter_ready` | 179 [164–197] | 186 [173–289] | 189 [178–200] |
| `first_rebuild_done` | 189 [173–204] | 199 [183–296] | 201 [187–210] |
| `first_encode_done` | 189 [173–204] | 199 [183–297] | 201 [187–210] |
| `first_submit_pipeline_wait` (duration) | 3 [3–5] | 145 [140–166] | 3 [3–5] |
| `first_frame_presented` | 202 [185–220] | 356 [333–449] | 214 [202–230] |
| **gap** `adapter_ready − pipeline_cache_restored` (`cache_loaded` where no blob was restored) | 161 [151–174] | 167 [160–197] | 168 [159–181] |
| `engine_surface_ms` | — | 15 [11–25] | 14 [10–22] |
| — of which `device_create_ms` | — | 1 [0–4] | 1 [0–7] |
| `cache_seed_ms` | — | 0 | 0 |
| `caps_probe_ms` | — | 0 | 0 |
| **`shader_modules_ms`** | — | 150 [143–176] | 151 [145–163] |
| `warm_up_start_ms` | — | 0 | 0 |
| `engine_rest_ms` | — | 0 | 0 [0–1] |
| `install_rest_ms` | — | 0 [0–2] | 0 [0–1] |
| sum of the seven install parts | — | 164 [157–195] | 166 [157–178] |
| gap − sum | — | 2 [2–3] | 2 [1–4] |
| `font_system_ms` (preinit thread) | — | 16 [9–27] | 15 [10–21] |
| `font_app_sync_ms` | — | 0 | 0 |
| `first_encode_done → first_frame_presented` | 13 [12–16] | 155 [150–176] | 14 [12–20] |

**Verdict — the owning sub-step.** `shader_modules_ms` is **90 % of the gap** (150 of 167 ms fresh, 151 of 168 warm; the seven parts sum to within 1–4 ms of the gap on every launch). The rest is `engine_surface_ms` 15 ms [10–25] (9 %; its `device_create_ms` 1 — the pre-created Vulkan device adopted — the remainder the capability gate and the swapchain configure), and `cache_seed_ms`, `caps_probe_ms`, `warm_up_start_ms`, `engine_rest_ms`, `install_rest_ms` all 0 — the driver-cache seed, the hardware-buffer probe and the warm-up spawn are excluded here as on the OnePlus 9. The twelve `create_shader_module` calls the OnePlus 9 block lists cost ≈12.5 ms each on the Snapdragon 765G, the same per-module shape at twice the price — naga's parse and validation of nine ≈1,400-line strip programs plus clear, copy and filter, where the September tree created four modules; the per-class programs landed between `36ef20db` and `87271063` (`d3f78a2d`, `c28b2a77`, `1d43d77c`), which is where this phone's `adapter_ready` 44 → 180 ms came from. `font_system_ms` 16 ms [9–27] on the preinit thread, `font_app_sync_ms` 0, `font_preinit_joined` 0 on every launch.

**The stamped build did not move the span, on this transport.** `adapter_ready` 186 ms median (fresh install, +4 %) and 189 (warm, +6 %) against 179; the gap 167 / 168 against 161 (+4 %); `first_frame_presented` on the warm series 214 ms [202–230] against 202 (+6 %), `first_encode_done → first_frame_presented` 14 ms against the criterion's 13.0; `am start -W` TotalTime 147 ms median of the kept launches (133/148/147; fresh-install block 150/152/145) against 141. All within ±10 % over the same wireless link the 2026-10-17 pass used. The fresh-install series' one outlier launch (run 1: `preinit_joined` 91 ms, `adapter_ready` 289, the first launch after the install) is in the ranges, not the medians.

**The fresh-install series never restored a blob** — the OnePlus 9 finding at this phone's price: no `pipeline_cache_restored` stamp on any of the 12 launches, `first_submit_pipeline_wait` 145 ms [140–166] (3 with a blob) and `first_frame_presented` 356 ms [333–449] against 214 warm — +142 ms on the first frame for an idle first screen killed in the foreground (`act_000001a1480df74b8iv9vHyR`); the warm column is the picture every launch after one rendering launch gets.

**Phase 2 target.** f2-01's remedy (a), as the OnePlus 9 block says; on this phone it is worth up to ≈110–115 of the 150 ms (nine strip programs at ≈12.5 ms), against the September `adapter_ready` of 44 ms (gap ≈22) as the bar.

### Device-open fix re-measure (2026-10-19)

**Method.** The f2-01 change — shader modules registered without being created at install, each created on first use by whichever thread asks, the first frame's four created side by side on a short-lived pool started at install (`perf/lazy-shader-modules` `3658806f` on `main` `42ef10d9`: `crates/frust-gpu/src/shader.rs`, `pipeline.rs`, `crates/frust-engine/src/gpu/pipelines.rs`, `renderer.rs`; no shader source, golden or pin changed) — read with the same recipe as the sub-step split above: the same profile APK as the OnePlus 9 leg (10,146,115 B, md5 `01945f7c…`), same phone and build (`UP1A.231105.001.B2`, serial redacted), wireless adb over the LAN with the radios as found (airplane off, Wi-Fi up — the recorded deviation of every Pixel 5 pass since 2026-10-17, `device_state.sh` warns on it), on battery (63 %, not charging), brightness fixed by the gate, refresh never pinned, no sampler, 33.0 → 33.5 °C. One 20 s S1 launch first so a pipeline-cache blob exists, then `run.sh s7 --runs 12 --duration 60`: all 12 launches carry a startup line and restored a blob. The **before** column is this build's baseline, the warm series of the sub-step split above (`main` `e89a639d`, same recipe, same session) re-read with the same script; `am start -W` is 4 cold launches, median of launches 2–4.

| Key (ms) | before — main `e89a639d`, 2026-10-19 warm series (the m1-05 block's last column) | after — `perf/lazy-shader-modules`, today, same recipe |
|---|---|---|
| `preinit_joined` | 20 [3–29] | 19 [5–33] |
| `cache_loaded` | 21 [3–30] | 20 [5–33] |
| `pipeline_cache_restored` | 21 [3–30] | 20 [5–33] |
| `adapter_ready` | 189 [178–200] | **40 [27–51]** |
| `first_rebuild_done` | 201 [187–210] | 56 [40–77] |
| `first_encode_done` | 201 [187–210] | 56 [40–77] |
| `first_submit_pipeline_wait` (duration) | 3 [3–5] | **35 [22–47]** |
| `first_frame_presented` | 214 [202–230] | **108 [97–125]** |
| **gap** `adapter_ready − pipeline_cache_restored` | 168 [159–181] | 19 [15–28] |
| `engine_surface_ms` | 14 [10–22] | 13 [11–24] |
| — of which `device_create_ms` | 1 [0–7] | 1 [0–13] |
| `cache_seed_ms` | 0 | 0 |
| `caps_probe_ms` | 0 | 0 |
| **`shader_modules_ms`** (now the registration alone) | 151 [145–163] | 1 [1–3] |
| `warm_up_start_ms` (now includes the pool's spawn) | 0 | 1 [0–3] |
| `engine_rest_ms` | 0 [0–1] | 0 [0–3] |
| `install_rest_ms` | 0 [0–1] | 0 [0–1] |
| sum of the seven install parts | 166 [157–178] | 17 [13–25] |
| gap − sum | 2 [1–4] | 2 [1–4] |
| `font_system_ms` (preinit thread) | 15 [10–21] | 12 [10–20] |
| `font_app_sync_ms` | 0 | 0 |
| `first_encode_done → first_frame_presented` | 14 [12–20] | **54 [33–62]** |
| `am start -W` TotalTime, median of launches 2–4 | 147 (133/148/147; launch 1 133) | 138 (138/145/131; launch 1 148) |

**Verdict against the card's bars.** `first_frame_presented` **108 ms median [97–125] against the bar of ≤ 172 (the 2026-09-06 median) — met**, 106 ms under the 214 ms baseline and 94 under 2026-10-17's 202. `adapter_ready` 40 [27–51] against September's 44 — back where it was: the gap is 19 ms [15–28] (its `engine_surface_ms` 13 is this phone's swapchain configure, unchanged), `shader_modules_ms` 1 (151 before), the seven install parts within 1–4 ms of the gap on every launch. **The post-encode guard is missed, by more than on the OnePlus 9:** `first_encode_done → first_frame_presented` 54 ms [33–62] against ≤ 25 (14 before), carried by `first_submit_pipeline_wait` 35 ms [22–47] (3 before): four modules of ≈ 12.5 ms each on a Snapdragon 765G with two big cores, started at install (≈ 40 ms) while the render thread's first rebuild runs to 56 ms, so the frame waits for creation the install used to pay 151 ms for. `am start -W` TotalTime 138 ms (138/145/131) against 147 before and 141 on 2026-10-17 — within the "not worse" bar, and again smaller than the engine's own move (−106 ms).

**What the miss says.** As on the OnePlus 9: the pool starts at install, ≈ 16 ms before the first frame's encode is done, and this chipset creates the four modules in ≈ 25–35 ms, so the frame waits for most of it. The lever for the residue — registering and creating the first frame's modules on the GPU pre-init thread, which owns the device ≈ 20 ms before the surface exists — is the follow-up card named on the OnePlus 9 block; it would make the post-encode span this phone's ≈ 14 ms again. The GO/NO-GO on merging the first-frame win with the guard missed is the plan owner's ruling, recorded on the card.

### Pre-init shader modules re-measure (2026-10-20) — NO-GO

**Method.** As the OnePlus 9 block of the same name: f2-03's branch `perf/preinit-shader-modules` `d3388e07` on `main` `45444997` (local, unmerged), the first frame's four shader modules started on the GPU pre-init thread ahead of the surface; the same APK (10,151,955 B, md5 `f4382875…`), same phone and build (`UP1A.231105.001.B2`, serial redacted), wireless adb with the radios as found (the recorded Pixel 5 deviation), on battery, brightness fixed by the gate, 29.0 → 30.2 °C, one 20 s S1 launch first, then `run.sh s7 --runs 12 --duration 60`, all 12 launches with a startup line and a restored blob. The **before** column is f2-01's series from the block above. The `frust-render shader pre-init:` line (logcat, 5 launches in the buffer) read **"adopted (0 of 4 first-frame modules created before the install)"** on four and "2 of 4" on one.

| Key (ms) | before — f2-01 (`perf/lazy-shader-modules`, 2026-10-19, the block above's after column) | after — `perf/preinit-shader-modules`, today, same recipe |
|---|---|---|
| `preinit_joined` | 19 [5–33] | 24 [7–40] |
| `cache_loaded` | 20 [5–33] | 24 [7–41] |
| `pipeline_cache_restored` | 20 [5–33] | 24 [7–41] |
| `adapter_ready` | 40 [27–51] | 36 [26–62] |
| `first_rebuild_done` | 56 [40–77] | 55 [36–89] |
| `first_encode_done` | 56 [40–77] | 55 [36–89] |
| `first_submit_pipeline_wait` (duration) | 35 [22–47] | **35 [21–54]** |
| `first_frame_presented` | 108 [97–125] | 107 [82–141] |
| **gap** `adapter_ready − pipeline_cache_restored` | 19 [15–28] | 16 [7–21] |
| `engine_surface_ms` | 13 [11–24] | 11 [6–17] |
| — of which `device_create_ms` | 1 [0–13] | 0 [0–5] |
| `cache_seed_ms` | 0 | 0 |
| `caps_probe_ms` | 0 | 0 |
| `shader_modules_ms` | 1 [1–3] | 0 |
| `warm_up_start_ms` | 1 [0–3] | 0 [0–3] |
| `engine_rest_ms` | 0 [0–3] | 0 [0–2] |
| `install_rest_ms` | 0 [0–1] | 0 [0–3] |
| sum of the seven install parts | 17 [13–25] | 13 [6–19] |
| gap − sum | 2 [1–4] | 2 [1–3] |
| `font_system_ms` (preinit thread) | 12 [10–20] | 11 [9–16] |
| `font_app_sync_ms` | 0 | 0 |
| `first_encode_done → first_frame_presented` | 54 [33–62] | **45 [33–69]** |
| `am start -W` TotalTime, median of launches 2–4 | 138 (138/145/131; launch 1 148) | 139 (139/146/136; launch 1 133) |

**Verdict — NO-GO.** `first_encode_done → first_frame_presented` 45 ms [33–69] against the ≤ 25 ms guard (54 with f2-01, 14 before it), `first_submit_pipeline_wait` 35 → 35: nothing moved where it matters; the 9 ms the median post-encode span lost is spread over the encode and queue steps with ranges that overlap entirely. `first_frame_presented` 107 [82–141] (bar ≤ 172 kept), `adapter_ready` 36 [26–62] against 40, `preinit_joined` 24 [7–40] against 19 [5–33], `am start -W` 139 against 138 and 2026-10-17's 141. The structural reason is the OnePlus 9 block's, sharper here: the device exists only at the pre-init join (≈ 24 ms), the install follows at once, four ≈ 12.5 ms modules on a Snapdragon 765G with two big cores take ≈ 25–35 ms of wall from their start, and the first encode at 55 ms still waits for most of them — the one launch with "2 of 4" created before the install is the one whose join came late (40 ms). Not merged; see the OnePlus 9 verdict for what is left.

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

### create 1k split (2026-10-19)

**Method.** Where the `create 1k` growth sits inside the frame, and whether the parallel strip walk is involved. Two profile APKs from `main` `772f8536` (the 2026-10-17 pass's engine plus the startup sub-step stamps of PR #97, nothing on the frame path), built on the Linux host with the checkout's own CLI from `benchmarks/frust_bench`: the default arm (`frust build apk --profile --target-platform android-arm64 --define FRUST_TRACE_RAW=1`, 10,137,291 B, md5 `e8b7fb09…`) and the `--define FRUST_ENGINE_NO_PARALLEL=1` arm (same byte count, md5 `7e272364…`, native library differs — `default_workers()` returns 1, the sequential walk). A third arm, outside the card's brief, isolates the engine's resident-run machinery: `--define FRUST_ENGINE_NO_GLYPH_RUN_CACHE=1` (md5 `fc2d2704…`), the kill switch that draws every glyph run through `glifo` on every frame instead of the slots the atlas policy remembered. Same phone and ROM as 2026-10-17 (`BP1A.250405.007`, serial redacted), USB, the matrix's prep (airplane on, Wi-Fi/Bluetooth off, 120 Hz pinned both ways, brightness 128, `dumpsys battery unplug` before each leg, 30 min timeout, keyguard dismissed; all restored afterwards), `run.sh s3 --runs 12 --duration 30` per arm with `device_state.sh`'s thermal gate (32.4 → 34.1 °C, 34.1 → 35.3 °C and 35.4 → 35.9 °C across the three legs, no cooldown wait), the first 2 runs discarded, no sampler. Every run captured 3,37x–3,40x raw lines. The 2026-09-06 and 2026-10-17 columns are the committed series (`git show 87271063:benchmarks/raw/oneplus9/frust_profile/s3/` and the tree's `raw/oneplus9/frust_profile/s3/`) re-read with the same script, which reproduces the two sections' published per-op rows exactly; today's raw trees stay out of the tree (the matrix's series remain the published ones).

**S3 overall series** (kept runs): default 4.29 / 12.62 / 19.61 ms p50 / p95 / p99 over 33,982 frames, `NO_PARALLEL` 4.31 / 12.69 / 19.72 over 33,922, `NO_GLYPH_RUN_CACHE` 4.40 / 12.87 / 19.87 over 33,983 — against 4.32 / 12.63 / 19.80 on 2026-10-17: the pass's series is reproduced, and the arm does not move the whole-series shape either.

Per-op reconcile-frame timing, p50 / p95 ms over the `[start, end)` op frames of the kept runs (n frames), by arm:

| Op | 2026-09-06 (`36ef20db` tree; series read from `git show 87271063:benchmarks/raw/…`) | 2026-10-17 (`87271063`) | `main` default | `main` `NO_PARALLEL` | `main` `NO_GLYPH_RUN_CACHE` |
|---|---|---|---|---|---|
| create 1k | 14.98 / 19.07 (341) | 21.38 / 25.91 (344) | 21.21 / 24.64 (342) | 21.62 / 25.25 (343) | 21.03 / 25.07 (341) |
| create 10k | 20.62 / 21.75 (346) | 19.37 / 20.38 (350) | 19.29 / 20.18 (350) | 19.33 / 20.13 (350) | 19.52 / 20.46 (350) |
| update every 10th of 10k | 15.57 / 19.20 (350) | 14.35 / 16.46 (350) | 14.45 / 16.45 (350) | 14.38 / 16.56 (350) | 14.38 / 16.51 (350) |
| swap | 14.54 / 19.14 (348) | 13.27 / 16.03 (340) | 13.12 / 15.11 (340) | 13.16 / 15.49 (340) | 14.14 / 17.85 (340) |
| clear | 15.18 / 17.17 (340) | 15.33 / 18.01 (340) | 15.28 / 17.67 (340) | 15.21 / 17.49 (340) | 14.95 / 17.45 (340) |

`create 1k` by frame of its cycle. The op window is exactly one frame (`start n=k`, `end n=k+1`); the script then runs 18 settle frames before `create 10k` opens at `k+20`. Frame 1 is the op frame `k`, frames 2 and 3 are `k+1` and `k+2`, "steady" is `k+3` up to the next op's start marker. Medians over every cycle of the kept runs, ms, as `rebuild / encode / submit` with the frame's `total_us` median in brackets (n frames: 341–343 per numbered row, 5,311–5,631 per steady row):

| Frame | 2026-09-06 | 2026-10-17 | `main` default | `main` `NO_PARALLEL` | `main` `NO_GLYPH_RUN_CACHE` |
|---|---|---|---|---|---|
| 1 (op frame `k`) | 1.66 / 0.05 / **12.56** [14.98] | 1.95 / 0.07 / **17.97** [21.38] | 1.88 / 0.07 / **17.82** [21.21] | 1.95 / 0.07 / **18.18** [21.62] | 1.89 / 0.07 / **17.80** [21.03] |
| 2 (`k+1`) | 0.33 / 0.06 / 7.63 [8.21] | 0.28 / 0.08 / 11.27 [11.89] | 0.27 / 0.08 / 10.98 [11.53] | 0.28 / 0.07 / 11.47 [12.13] | 0.27 / 0.08 / 11.17 [11.80] |
| 3 (`k+2`) | 0.37 / 0.03 / 5.51 [6.17] | 0.28 / 0.03 / 6.44 [7.05] | 0.30 / 0.03 / 6.43 [7.07] | 0.29 / 0.03 / 6.44 [7.07] | 0.30 / 0.03 / 6.43 [7.07] |
| steady (`k+3` … next op) | 0.44 / 0.04 / 5.37 [7.09] | 0.45 / 0.03 / 2.70 [4.32] | 0.46 / 0.03 / 2.69 [4.10] | 0.46 / 0.03 / 2.68 [4.13] | 0.46 / 0.03 / 3.01 [4.53] |

The steady row hides a step. Submit by position in the 20-frame cycle (p50 ms; positions 4–9 then 10–12): 2026-09-06 4.92 / 4.89 / 5.54 / 5.97 / 6.19 / 6.17 then 3.94 / 4.28 / 5.33; `main` default 5.21 / 4.99 / 5.56 / 5.90 / 6.01 / 6.03 then **1.90 / 2.54 / 2.65**; `NO_PARALLEL` 5.23 / 5.05 / 5.60 / 5.84 / 5.97 / 6.03 then 1.89 / 2.54 / 2.62; `NO_GLYPH_RUN_CACHE` 5.25 / 4.99 / 5.57 / 5.79 / 5.99 / 6.03 then 2.24 / 2.60 / 2.95. Summed per cycle over the frames present (the kept runs' 350 cycles each; 20 frames per cycle in September, 19 today — one scene per cycle is now overtaken and never rendered):

| Series | submit, frames 1–2 | frames 3–9 | frames 10–20 | whole cycle (submit) | whole cycle (`total_us`) |
|---|---|---|---|---|---|
| 2026-09-06 | 21.4 ms | 39.1 | 54.2 | 114.3 | 150.4 |
| 2026-10-17 | 28.8 | 40.3 | 25.0 | 93.4 | 116.5 |
| `main` default | 28.6 | 40.1 | 25.0 | 92.4 | 115.4 |
| `main` `NO_PARALLEL` | 29.1 | 40.5 | 24.6 | 92.8 | 115.6 |
| `main` `NO_GLYPH_RUN_CACHE` | 28.6 | 40.1 | 28.1 | 96.1 | 119.0 |

GPU, from the `gpu_q=1` readings inside the cycle (they lag their line, so the cycle's maximum is the op frame's own pass): the largest `gpu_main` reading in a `create 1k` cycle is 0.59 ms p50 / 1.03 p95 on the default arm and 0.59 / 0.87 on `NO_PARALLEL` (2.06 / 2.16 in September); `gpu_prepass` is 0 in every cycle (p95 0.17 ms) — no atlas replay of any size. `encode_us` is the engine-direct arm's memcpy and reads 0.07 ms on the op frame on every series. The engine's own 60-frame `frust-perf enc` windows read the same on both arms (medians: walk 0.55–0.56 ms, glyphs 0.16, upload 0.05, 408 strips and 13,648 alphas per frame against 5,089 / 123,776 in September; `glyph_misses` 0 after the first window of a run, `staging_busy`/`staging_too_large` 0, `bind_groups` 0) — the per-frame op cost is above their medians, so they bound what a typical frame costs, not what the op frame does.

**Verdict — where it sits.** The +6.4 ms on the `create 1k` row (14.98 → 21.38 ms, +43 %) is the **op frame's own `submit`** (12.56 → 17.97, +5.4 ms; rebuild +0.3, layout and paint +0.2) **plus the frame after it** (submit 7.63 → 11.27, +3.6 ms). The seven frames after those are flat against September (5–6 ms each), and from the tenth frame of the cycle on every frame is 2.7 ms cheaper (5.4 → 2.6 ms) — so a whole `create 1k` cycle costs 22 ms *less* submit than in September (114 → 92 ms) while the one frame the criterion measures costs 5 ms *more*. It is a first-frames cost after a bulk insert, not a per-frame cost of the op. The 9-frame shape (op frame + 8) matches the atlas policy's `SETTLE_FRAMES = 8` demotion window (`crates/frust-engine/src/text/atlas_policy.rs`; already in the September tree), which opens on an observed font-size change and draws that size through the slow path until it closes — the table's size reappears after 18 empty frames and the policy treats that as a change; September paid that cost on every frame, so the window was invisible there and is exposed now by the cheap steady frames. The move is **not GPU work** (no reading above 1 ms anywhere in the cycle; September's were 2 ms), **not the parallel strip walk** (`NO_PARALLEL` leaves every row where it is: op frame 18.18 vs 17.82 ms, frame 2 11.47 vs 10.98, steady 2.68 vs 2.69 — the +0.4 ms is the share the helpers took off the walk), and **not the resident-run cache** (`NO_GLYPH_RUN_CACHE` leaves frames 1–9 where they are — 17.80 / 11.17 / 6.43 … 6.03 ms — and costs the steady frames +0.3 ms, 2.69 → 3.01, its whole contribution). What survives all three arms is CPU time on the render thread's submit, outside the strip walk and outside the engine's GPU passes, on exactly the two frames that first carry the new rows; of the engine mechanisms new since the September tree (glyph quads, the staging ring, the parallel walk, the resident-run cache — all absent at `36ef20db`), the two not yet excluded are the glyph-quad lowering (one run record plus rect instances per fresh run) and the staging-ring upload of the first non-resident frames.

**Phase 2 target.** Two candidates, both engine-side, both on the first frames after a bulk insert rather than per frame, and both measurable with existing kill switches before any fix: (1) the settle window opening for a size that merely *reappears* after an empty scene — frames 2–9 at 5–6 ms would fall to the 2.6 ms steady cost, ≈24 ms per cycle, the larger lever; (2) the op frame's own +5.4 ms and frame 2's +3.6 ms, to be split with `FRUST_ENGINE_NO_GLYPH_QUADS=1` and `FRUST_ENGINE_NO_STAGING_RING=1` arms on this recipe (one leg each, the S3 block only), and with a per-frame engine stamp if neither moves it (the `enc` line is a 60-frame median and cannot see one frame). Neither candidate is a per-frame GPU cost and neither is the walk, so the item is not a documented trade yet: `act_000001a14dde1e09f8S3m5Hm` stays open for Phase 2 with (1) as the first card; if Phase 2 declines both, the trade to record is "the op frame +5 ms, the cycle −22 ms".

### S7

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | ~128 ms (128/123/130; first of 4 discarded) | ~402 ms (402/402/403; first of 4 discarded) |
| Framework-reported first-frame span | ~114 ms median (`first_frame_presented`, 108–125 across 12 launches; `adapter_ready` median 100 ms) | ~74 ms median (`first_frame_ms`, 62–85 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | 0 samples | 0 samples |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~82.5 MiB (82.3–83.0, 10 kept snapshots) | ~133.8 MiB (133.5–134.0, 10 kept snapshots) |

**Frust starts ~3.1× faster and idles ~38 % lighter; its framework-reported first frame is longer than in September.** External cold start 128 vs 402 ms (`am start -W` TotalTime; 128 on 2026-09-06), idle TOTAL PSS 82.5 vs 133.8 MiB (114.1 before: −27.6 %), idle CPU 0 samples on both. The Frust first-frame span is 114 ms median (95 before) against Flutter's 74, measured from different entry points as always; inside it, `first_encode_done` → `first_frame_presented` fell 53.5 → 10.5 ms [10–12] with the persisted pipelines, while `adapter_ready` rose 36 → 100 ms (see the criteria block above).

### Startup sub-step split (2026-10-19)

**Method.** Where the `adapter_ready` move sits, read with the sub-step stamps of PR #97 (`main` `e89a639d`, the 2026-10-17 engine plus the stamps and the PR #98/#99 benchmark notes — nothing on the startup path). One profile APK built on the Linux host from the checkout's own CLI (`frust build apk --profile --target-platform android-arm64 --define FRUST_TRACE_RAW=1` in `benchmarks/frust_bench`; 10,136,347 B, md5 `c4ae3d9a…`), installed fresh; same phone and ROM as 2026-10-17 (`BP1A.250405.007`, serial redacted), USB, the matrix's prep (airplane on, Wi-Fi/Bluetooth off, 120 Hz pinned both ways, brightness 128, `dumpsys battery unplug` before the leg, 30 min timeout, keyguard dismissed; all restored afterwards), no sampler. Two series of `run.sh s7 --runs 12 --duration 60` (`device_state.sh` gate, 31.9 → 33.1 °C and 33.3 → 33.6 °C, no cooldown wait): the **fresh-install series** first, then — after one 20 s S1 launch that renders continuously — the **warm series**, because the first series turned out to run every launch without a persisted pipeline-cache blob (below). The warm series ran without the on-battery spoof (`run.sh`'s own exit reset it after the S1 launch; the phone is USB-powered on both passes) — a declared deviation of that series only. The 2026-10-17 column is the committed S7 series (`raw/oneplus9/frust_profile/s7/`) re-read with the same script, which reproduces the section's published medians; today's raw trees stay out of the tree. All 12 launches of every series carry a startup line (medians and [min–max] over all 12, as the S7 rows above). The ten added keys are **durations** (`…_ms`), truncated to whole milliseconds; the existing keys are offsets from `init_entry`.

| Key (ms) | 2026-10-17 (`87271063`, matrix S7 block, blob present) | today, fresh install (no blob on any launch) | today, after one rendering launch (blob restored) |
|---|---|---|---|
| `preinit_joined` | 20 [15–29] | 25 [16–41] | 28 [17–37] |
| `cache_loaded` | 20 [15–29] | 26 [16–41] | 28 [18–37] |
| `pipeline_cache_restored` | 20 [15–29] | **absent on all 12** | 28 [18–37] |
| `adapter_ready` | 100 [95–112] | 107 [101–125] | 111 [101–121] |
| `first_rebuild_done` | 103 [98–115] | 109 [104–129] | 113 [103–123] |
| `first_encode_done` | 103 [98–115] | 109 [104–129] | 113 [103–123] |
| `first_submit_pipeline_wait` (duration) | 3 [3–6] | 70 [65–71] | 3 [1–3] |
| `first_frame_presented` | 114 [108–125] | 189 [178–205] | 124 [114–134] |
| **gap** `adapter_ready − pipeline_cache_restored` (`cache_loaded` where no blob was restored) | 81 [78–83] | 82 [80–85] | 83 [80–85] |
| `engine_surface_ms` | — | 5 [5–6] | 5 [5–6] |
| — of which `device_create_ms` | — | 0 | 0 [0–1] |
| `cache_seed_ms` | — | 0 | 0 |
| `caps_probe_ms` | — | 0 | 0 |
| **`shader_modules_ms`** | — | 76 [73–78] | 75 [73–79] |
| `warm_up_start_ms` | — | 0 | 0 |
| `engine_rest_ms` | — | 0 | 0 |
| `install_rest_ms` | — | 0 | 0 |
| sum of the seven install parts | — | 81 [78–83] | 81 [78–84] |
| gap − sum | — | 2 [1–2] | 2 [1–3] |
| `font_system_ms` (preinit thread) | — | 8 [8–13] | 8 [7–17] |
| `font_app_sync_ms` | — | 0 | 0 |
| `first_encode_done → first_frame_presented` | 11 [10–14] | 78 [74–80] | 11 [9–12] |

**Verdict — the owning sub-step.** `shader_modules_ms` is **92 % of the gap on the fresh-install series (76 of 82 ms) and 90 % warm (75 of 83)**; the seven install parts sum to within 1–3 ms of the gap on every launch — the truncation of seven durations, nothing unaccounted for. It is the only part above 6 ms: `engine_surface_ms` 5 (its `device_create_ms` 0 — the Vulkan device was pre-created on the preinit thread and `ensure_device` only adopts it), `cache_seed_ms` 0 with and without a blob, `caps_probe_ms` 0, `warm_up_start_ms` 0, `engine_rest_ms` 0, `install_rest_ms` 0. So the three candidates the criteria block named — driver-cache seeding, the hardware-buffer probe, the warm-up worker starting beside device creation — are all excluded, together: they cost nothing measurable. The stamp covers `ShaderLibrary::new` plus `EngineShaders::register_family` (`crates/frust-engine/src/gpu/pipelines.rs`), which today is **twelve `create_shader_module` calls, not four**: the strip program's Uber variant, its seven class specialisations (`PaintClass::SPECIALISED`: solid-coverage, gradient, image, image-filtered, blurred-rect, layer-composite, glyph), the atlas-layer Uber program of the manual family (one more under the default image-sampler family), and the clear, copy and filter modules — each strip program the `helpers.wgsl` + `strip.wgsl` source (≈1,400 lines before the per-class specialisation drops the blocks a class does not use) parsed and validated by naga at module creation, ≈6 ms a module on this chipset. The September tree (`36ef20db`) created four modules (strip, clear, copy, filter); the per-class strip programs arrived with `d3f78a2d` (2026-10-13), the sampler family with `c28b2a77` and the glyph class with `1d43d77c` (2026-10-14), all between `36ef20db` and the `87271063` the 2026-10-17 pass ran — which is where `adapter_ready` 36 → 100 ms came from. The font half of the stamps reads `font_system_ms` 8 ms [7–17] on the preinit thread (parley's `FontContext::new`) and `font_app_sync_ms` 0 (the bench app registers no fonts), neither on the critical path (`font_preinit_joined` 0 on every launch).

**The stamped build did not move the span.** `adapter_ready` 107 ms median (fresh install, +7 %) and 111 (warm, +11 %) against 100 on 2026-10-17; the gap itself is 82 / 83 against 81 (+2 %), and the warm series' extra sits before it, in `preinit_joined` (28 vs 20 ms — the join of the GPU pre-init thread, which also reads the blob; a between-session move of the join, read with the spoof deviation above, not of the install). `first_frame_presented` on the warm series is 124 ms [114–134] against 114 (+9 %), with `first_encode_done → first_frame_presented` 11 ms, the criterion's 10.5 — the first frame after encode is where it was. `am start -W` TotalTime 127 ms median of the kept launches (129/127/126; fresh-install block 123/119/120) against 128.

**Finding on the side — the fresh-install series never restored a blob.** On all 12 launches after the fresh install the startup line carries no `pipeline_cache_restored` stamp, `first_submit_pipeline_wait` reads 70 ms [65–71] (3 with a blob) and `first_frame_presented` 189 ms [178–205]: every launch compiled the first frame's pipelines cold. The blob is written once the warm-up has built its list, **after a later frame** (`CachePersistence::after_frame` in `crates/frust-shell-android/src/jni_glue.rs`), on surface-destroy or on pause; S7 deliberately requests no frame after its first, the warm-up finishes after that frame, and the harness's `am force-stop` reaches neither pause nor surface-destroy — so an idle first screen that is killed in the foreground leaves no blob, launch after launch. On 2026-10-17 the S7 block ran after six rendering blocks that had written it. One rendering launch fixes it for every launch that follows (the warm column), and a backgrounded app pauses and persists; the exposure is the launch sequence "static first screen → killed" — already filed as the minor `act_000001a1480df74b8iv9vHyR` ("Static first screen killed in the foreground before any later frame or surface destroy leaves no blob"), now with its cost measured against the same build's warm series: +65 ms on the first frame here (189 against 124), +142 ms on the Pixel 5 (356 against 214).

**Phase 2 target.** f2-01's remedy (a): take the strip-program module creation off the install path — the render thread creates only the modules the first frame's own variants need (which ones, the warm-up list says), the other class specialisations are created on the warm-up worker that already builds their pipelines, or the family collapses into one module with several entry points if the cost is per-module overhead rather than per-line parsing (to be split on the Xiaomi 12 / iPhone SE leg as the card says). At ≈6 ms a module the nine strip programs are ≈55–60 of the 76 ms; the September `adapter_ready` of 36 ms (gap ≈16) is the bar, not a projection.

### Device-open fix re-measure (2026-10-19)

**Method.** The f2-01 change — shader modules registered without being created at install, each created on first use by whichever thread asks, the first frame's four created side by side on a short-lived pool started at install (`perf/lazy-shader-modules` `3658806f` on `main` `42ef10d9`: `crates/frust-gpu/src/shader.rs`, `pipeline.rs`, `crates/frust-engine/src/gpu/pipelines.rs`, `renderer.rs`; no shader source, golden or pin changed) — read with the same recipe as the sub-step split above: one profile APK from the branch (`frust build apk --profile --target-platform android-arm64 --define FRUST_TRACE_RAW=1`; 10,146,115 B, md5 `01945f7c…`), the matrix's prep (airplane on, Wi-Fi/Bluetooth off, 120 Hz pinned both ways, brightness 128, `dumpsys battery unplug`, 30 min timeout; all restored afterwards), same phone and ROM (`BP1A.250405.007`, serial redacted), USB, no sampler, 31.6 → 33.7 °C. One 20 s S1 launch first so a pipeline-cache blob exists (the fresh-install trap above), the battery spoof re-armed after it, then `run.sh s7 --runs 12 --duration 60` (`device_state.sh` gate, charger off, airplane on): all 12 launches carry a startup line and restored a blob. The **before** column is this build's baseline, the warm series of the sub-step split above (`main` `e89a639d`, same recipe, same session) re-read with the same script; `am start -W` is 4 cold launches, median of launches 2–4.

| Key (ms) | before — main `e89a639d`, 2026-10-19 warm series (the m1-05 block's last column) | after — `perf/lazy-shader-modules`, today, same recipe |
|---|---|---|
| `preinit_joined` | 28 [17–37] | 24 [19–36] |
| `cache_loaded` | 28 [18–37] | 24 [19–36] |
| `pipeline_cache_restored` | 28 [18–37] | 24 [19–36] |
| `adapter_ready` | 111 [101–121] | **32 [27–46]** |
| `first_rebuild_done` | 113 [103–123] | 35 [30–50] |
| `first_encode_done` | 113 [103–123] | 35 [30–50] |
| `first_submit_pipeline_wait` (duration) | 3 [1–3] | **21 [18–28]** |
| `first_frame_presented` | 124 [114–134] | **67 [64–80]** |
| **gap** `adapter_ready − pipeline_cache_restored` | 83 [80–85] | 8 [7–11] |
| `engine_surface_ms` | 5 [5–6] | 5 [5–7] |
| — of which `device_create_ms` | 0 [0–1] | 0 [0–1] |
| `cache_seed_ms` | 0 | 0 |
| `caps_probe_ms` | 0 | 0 |
| **`shader_modules_ms`** (now the registration alone) | 75 [73–79] | 1 |
| `warm_up_start_ms` (now includes the pool's spawn) | 0 | 0 [0–1] |
| `engine_rest_ms` | 0 | 0 [0–2] |
| `install_rest_ms` | 0 | 0 |
| sum of the seven install parts | 81 [78–84] | 6 [6–9] |
| gap − sum | 2 [1–3] | 2 [1–3] |
| `font_system_ms` (preinit thread) | 8 [7–17] | 8 [7–13] |
| `font_app_sync_ms` | 0 | 0 |
| `first_encode_done → first_frame_presented` | 11 [9–12] | **32 [28–39]** |
| `am start -W` TotalTime, median of launches 2–4 | 127 (129/127/126; launch 1 121) | 135 (135/134/135; launch 1 136) |

**Verdict against the card's bars.** `first_frame_presented` **67 ms median [64–80] against the bar of ≤ 95 (the 2026-09-06 median) — met**, 57 ms under the 124 ms baseline and 47 under 2026-10-17's 114. `adapter_ready` 32 [27–46] against September's 36 — back where it was: the gap is 8 ms [7–11] (September ≈ 16), `shader_modules_ms` 1 (75 before; the bar was "at least halved"), and the seven install parts still sum to within 1–3 ms of the gap on every launch, so nothing moved into an unstamped place. **The post-encode guard is missed:** `first_encode_done → first_frame_presented` 32 ms [28–39] against ≤ 25 (11 before), all of it `first_submit_pipeline_wait` 21 ms [18–28] (3 before) — the first frame now waits for the pool's module creation, which starts at install (≈ 30 ms) and lands after the frame's encode (35 ms): four modules of ≈ 6 ms each side by side on a Snapdragon 888 whose render, preinit and UI threads are busy at the same moment, then the frame's own pipelines from the seeded driver cache. The time the install no longer pays (75 ms) is not spent elsewhere: the frame absorbs ≈ 18 ms of it and the rest is gone. `am start -W` TotalTime 135 ms (135/134/135) against 127 before and 128 on 2026-10-17: +7 ms, **outside the "not worse" bar**, and it did not follow the 57 ms move of `first_frame_presented` at all — TotalTime is bounded by something other than the engine's first present on this phone (the activity's own first window draw is the usual candidate; the pool's four threads overlapping that draw is the plausible cost) — recorded, not attributed.

**Strategy arms.** One S7 launch each on a `--define FRUST_FIRST_FRAME_WARM_UP=await` and `=finish` APK, after a 20 s S1 launch each (first frames 253 / 287 ms on those rendering launches, which build the whole list): `await` `adapter_ready` 34, `first_submit_pipeline_wait` 78, `first_frame_presented` 125 ms; `finish` 29 / 79 / 121 ms — both present a first frame well inside the 1 s timeout, paying the whole list before it as designed, with `shader_modules_ms` 1 on both; the three meanings are unchanged. `steal` stays the default.

**What the miss says.** The pool starts where the modules used to be created, at install, and that is ≈ 5 ms before the first frame asks for them; on this phone the frame therefore waits ≈ 18 ms for creation it previously found done (paid before `adapter_ready` instead). The lever the card names for a residue — registering and creating the first frame's modules on the GPU pre-init thread, which on Android owns the device ≈ 20 ms before the surface exists (`spawn_gpu_preinit`, `crates/frust-shell-android/src/jni_glue.rs`) — is outside this card and is filed as a follow-up; it would put the modules ahead of the install and return the post-encode span to its ≈ 11 ms. The GO/NO-GO on merging the first-frame win with the guard missed is the plan owner's ruling, recorded on the card.

### Pre-init shader modules re-measure (2026-10-20) — NO-GO

**Method.** f2-03, the follow-up to the guard miss above: the first frame's four shader modules started on the Android GPU pre-init thread, ahead of the surface, instead of at install (branch `perf/preinit-shader-modules` `d3388e07` on `main` `45444997`, kept local and unmerged: the pre-init thread builds the `SurfaceRenderer` beside the device and prepares the engine's shader family on it, the install adopts the prepared library when the device and family match, and the engine skips its own pool for modules already created or in flight). Same recipe as the block above — one profile APK from the branch (10,151,955 B, md5 `f4382875…`), the matrix's prep, same phone and ROM (`BP1A.250405.007`, serial redacted), USB, 31.4 → 32.7 °C, one 20 s S1 launch first, the battery spoof re-armed, then `run.sh s7 --runs 12 --duration 60`, all 12 launches with a startup line and a restored blob. The **before** column is f2-01's series from the block above (same session's recipe on `perf/lazy-shader-modules`); `am start -W` is 4 cold launches, median of launches 2–4. The `frust-render shader pre-init:` line (logcat, 5 launches in the buffer) read **"adopted (0 of 4 first-frame modules created before the install)"** on every one.

| Key (ms) | before — f2-01 (`perf/lazy-shader-modules`, 2026-10-19, the block above's after column) | after — `perf/preinit-shader-modules`, today, same recipe |
|---|---|---|
| `preinit_joined` | 24 [19–36] | 27 [16–41] |
| `cache_loaded` | 24 [19–36] | 27 [17–41] |
| `pipeline_cache_restored` | 24 [19–36] | 27 [17–41] |
| `adapter_ready` | 32 [27–46] | 34 [24–48] |
| `first_rebuild_done` | 35 [30–50] | 36 [29–52] |
| `first_encode_done` | 35 [30–50] | 36 [29–52] |
| `first_submit_pipeline_wait` (duration) | 21 [18–28] | **17 [15–23]** |
| `first_frame_presented` | 67 [64–80] | 64 [52–85] |
| **gap** `adapter_ready − pipeline_cache_restored` | 8 [7–11] | 7 [6–8] |
| `engine_surface_ms` | 5 [5–7] | 5 [5–7] |
| — of which `device_create_ms` | 0 [0–1] | 0 [0–1] |
| `cache_seed_ms` | 0 | 0 |
| `caps_probe_ms` | 0 | 0 |
| `shader_modules_ms` | 1 | 0 |
| `warm_up_start_ms` | 0 [0–1] | 0 |
| `engine_rest_ms` | 0 [0–2] | 0 |
| `install_rest_ms` | 0 | 0 |
| sum of the seven install parts | 6 [6–9] | 5 [5–7] |
| gap − sum | 2 [1–3] | 1 [1–2] |
| `font_system_ms` (preinit thread) | 8 [7–13] | 8 [6–9] |
| `font_app_sync_ms` | 0 | 0 |
| `first_encode_done → first_frame_presented` | 32 [28–39] | **28 [23–33]** |
| `am start -W` TotalTime, median of launches 2–4 | 135 (135/134/135; launch 1 136) | 129 (129/131/129; launch 1 133) |

**Verdict — NO-GO, the lever cannot reach.** `first_encode_done → first_frame_presented` 28 ms [23–33] against the ≤ 25 ms guard (32 with f2-01, 11 before it): `first_submit_pipeline_wait` 21 → 17 ms, 4 ms of the 18 the frame absorbed. `first_frame_presented` 64 [52–85] (bar ≤ 95 kept), `adapter_ready` 34 [24–48] against f2-01's 32 (ranges overlapping), `preinit_joined` 27 [16–41] against 24 [19–36] — the registration (≈ 1 ms) and four thread spawns now sit on the pre-init thread before its join, a growth inside the noise but in the wrong direction; `am start -W` 129 (129/131/129) against f2-01's 135 and 2026-10-17's 128. The reason is structural, not a defect of the branch: a `wgpu::ShaderModule` needs the device, the pre-init thread owns one only at the end of its work, and `create_handle`'s join returns at that same moment — so the pool starts a few milliseconds before the install however early the thread was spawned, and on every launch in the logcat buffer none of the four modules existed when the install ran. The pre-init thread finishes device creation at ≈ 24–27 ms (`preinit_joined`), the install follows at once, the first encode lands at 36 ms, and four ≈ 1,400-line modules on four busy big cores take ≈ 10–17 ms of wall from their start — which is the wait. Keeping the code would buy ≈ 4 ms on this phone and nothing on the Pixel 5 (below) for a pre-init thread that also builds the renderer and reads the engine's configuration at library load; it is not merged, and f2-01's guard miss is recorded in `docs/LIMITATIONS.md` (`android-first-frame-carries-shader-module-creation`). The levers left are outside the device: WGSL parsed and validated without a device (naga IR handed to `create_shader_module`, which needs wgpu's `naga-ir` surface and a pin decision) or a smaller first-frame program set — a separate plan's question.

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

**Status:** run 2026-10-17 17:05–19:22 UTC, **12 runs × 30 s per scenario per app (S7: 60 s runs — the protocol's idle window, on iOS too from this pass), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** **Frust = PROFILE build of `20e0aa2e`** (the build the Xiaomi 12 section describes: engine, render, GPU, shell and widget crates byte-identical to the OnePlus 9 / Pixel 5 pass's `87271063`, `frust-core` with PR #83's readability refactor — deviation 2; `frust build ios --profile --define FRUST_TRACE_RAW=1` with the checkout's own CLI, raw format v4; one signed profile `Runner.app`, 13.0 MiB on disk, serves every scenario through the launch env `FRUST_BENCH_SCENARIO`), **Flutter = profile build** (`flutter build ios --profile --dart-define=SCENARIO=<sN>`, Flutter 3.47.2 under Xcode 27.0), one `flutter_sN.app` per scenario (24.3 MiB each) installed at the start of its block. Raw series under `raw/iphone_se/frust_profile/<sN>/` and `raw/iphone_se/flutter/<sN>/` (192 run logs, 16 stats.txt; S7 adds coldstart.txt and run-NN.sysmon.txt for runs 3/5/7/9/11; no PSS snapshots on iOS). One unattended `matrix.sh` session from the Mac, all 16 blocks captured and graded `ok` at the first attempt — no retries (2026-09-06 re-ran two). **This pass supersedes the 2026-09-06 pass on this device** (its section: `git show 20e0aa2e:benchmarks/RESULTS.md`).

- Chipset: Apple A13 Bionic (arm64e); iPhone SE 2nd gen (iPhone12,8); UDID and serial redacted (`device.txt` prints neither).
- OS: iOS 26.7.1 (23H30) — 26.6.1 on 2026-09-06; Developer Mode on; USB-connected `xcrun devicectl` transport (network-paired in September), Xcode 27.0 (27A266a) / devicectl 642.16.
- Display: 60 Hz panel, single mode. **The 8.33 ms column is N/A on this device** (summarize.py still prints it; it is not discussed).
- Refresh opt-in (§6): no >60 Hz mode exists, so the gate is moot; Frust ran ~60.4–60.7 fps and Flutter ~60.5–60.8 in every frame scenario — Flutter's frame-timing count now matches the panel (18,165–18,253 per 300 s against 20,092–20,498 in September; deviation 6).
- Environment: brightness, airplane mode, Wi-Fi and Bluetooth stay **uncontrolled** (no iOS CLI); the captures' status bar shows airplane mode on with Wi-Fi on throughout. Charger: USB-connected throughout (devicectl and power); battery level **100 % at every block** (`blocks.tsv`, read over lockdown).
- Thermal, recorded for the first time on this device: battery temperature **30.5–31.9 °C** across the 16 blocks (session 30.7 → 30.6 °C), fixed 60 s inter-block cooldown; the 38 °C / 120 s gate never engaged. 30 s blocks 376–403 s wall (Flutter's include the per-scenario install), the S7 blocks 840 / 890 s with the Instruments saves.
- Rig: Xcode.app open on the Mac for the whole run (the Instruments precondition; `sysmon: Instruments online` logged at 17:05:19). Instruments recorded the last 15 s of every other kept S7 run (3/5/7/9/11), 15–16 sysmon samples per recording and no short capture; the driver waited for an in-flight save before runs 5 and 9 (Frust) and 7, 9 and 11 (Flutter).
- Toolchain: rustc 1.98.1 (pinned), `wgpu` 30.0.1 (pinned), Flutter 3.47.2 stable / Dart 3.13.2, Xcode 27.0 (26.2 in September).
- Visual gate (driver screencap 8 s into run 1 of every block, all 16 reviewed on a contact sheet plus band luma — the first iOS pass with one): **content PASS on 15 blocks** — every scenario renders on both apps and no keyboard or system window intrudes on any capture (Flutter bottom-band luma 19–40; Frust's 102–157 is its own scenario tab bar). **Flutter S5 FAIL on content:** the capture shows one of three visible cells decoded and two grey placeholders (mid-band luma 73 against Frust's 120 with its full decoded column), and a 45 s re-check on the same build after the matrix (screenshots at 8/20/32/45 s) never showed more than one decoded cell — deviation 1.
- Sanitization: driver staging `RAW_OK` (192 run logs, 16 stats.txt, 10 sysmon.txt, 2 coldstart.txt), whitelist self-check passed; no UDID, serial or address in any staged file.
- iOS conventions (unchanged): Frust `total_us` (rows marked †) folds in the CADisplayLink acquire/vsync wait (`acquire` p50 4.2–14.4 ms this pass; S4's is bimodal, see its note), so † totals pin to the 16.67 ms cadence; **the "CPU work" row is `total − acquire` = rebuild+layout+paint+encode+submit** and is the per-frame comparison row. Flutter's `totalSpan` goes negative under load, so its frame total is `build_us+raster_us`. **For the engine, `submit` waits on the GPU, so that row is CPU+GPU on the Frust side while Flutter's `build+raster` is CPU only — the two are not like-for-like, and no per-frame winner is declared from them; Frust's real GPU time is the `gpu_total` figure in each pass-breakdown line (Metal timestamps, `gpu_q=1`).**

### S1

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 16.81 | 18.46 | 18.94 | 41.98 | 11,385 (62.61%) | 18,076 (99.41%) | 18,184 (~60.6 fps avg) |
| Frust CPU work (total − acquire wait) | 5.57 | 6.70 | 7.09 | 14.35 | 0 (0.00%) | 9 (0.05%) | 18,184 (~60.6 fps avg) |
| Flutter (profile) | 5.05 | 5.49 | 5.68 | 20.99 | 8 (0.04%) | 10 (0.06%) | 18,165 (~60.5 fps avg) |

Frust pass p50 (ms): rebuild 0.09, layout 0.00, paint 0.16, encode 0.10, acquire 11.27, submit 5.22; layout_us>0 on 18,116/18,184 frames; GPU (gpu_q=1 on 18,144) total p50/p95 1.93/1.96 ms

**Flutter carries less CPU work per frame; both hold the panel.** Work row 5.57 vs 5.05 ms (p95 6.70 vs 5.49), 0.00 % vs 0.04 % of frames over the 60 Hz budget. Frust's GPU pass fell 4.38 → 1.93 ms since September but the work row only moved 5.79 → 5.57: the S1 encode window reads 4.04 ms p50 (4.71 before), 3.16 ms of it the strip walk, and that CPU walk — not the GPU — is what the A13 pays on this scene. The plan's 1.5 ms S1 encode bar is missed (criteria block).

### S2

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 20.34 | 21.41 | 23.37 | 33.55 | 16,060 (88.58%) | 16,250 (89.63%) | 18,131 (~60.4 fps avg) |
| Frust CPU work (total − acquire wait) | 5.96 | 7.03 | 9.26 | 18.95 | 1 (0.01%) | 279 (1.54%) | 18,131 (~60.4 fps avg) |
| Flutter (profile) | 4.93 | 5.61 | 5.90 | 10.86 | 0 (0.00%) | 10 (0.16%) | 6,421 (~21.4 fps avg) |

Frust pass p50 (ms): rebuild 0.08, layout 3.57, paint 0.21, encode 0.04, acquire 14.40, submit 2.10; layout_us>0 on 18,131/18,131 frames; GPU (gpu_q=1 on 18,091) total p50/p95 1.02/1.03 ms

**Frust pays more per frame and paints far more of them.** Work row 5.96 vs 4.93 ms (p95 7.03 vs 5.61); Frust repaints continuously at ~60.4 fps (18,131 frames) while Flutter's scroll paint is event-driven (6,421 frames; deviation 9), so the per-frame figures compare and the frame counts do not. Frust's row fell 6.47 → 5.96 with submit 3.00 → 2.10 and the GPU pass 3.08 → 1.02 ms, while the per-frame layout rose 2.53 → 3.57 ms and is now 60 % of the row (the Xiaomi 12 moved the same way, 1.37 → 1.93); the plan's 5.0 ms bar is missed by 0.96 ms.

### S4

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 6.64 | 16.88 | 17.05 | 22.70 | 5,527 (30.41%) | 7,233 (39.79%) | 18,177 (~60.6 fps avg) |
| Frust CPU work (total − acquire wait) | 2.37 | 2.57 | 2.71 | 10.13 | 0 (0.00%) | 5 (0.03%) | 18,177 (~60.6 fps avg) |
| Flutter (profile) | 1.34 | 1.52 | 1.65 | 7.53 | 0 (0.00%) | 0 (0.00%) | 18,253 (~60.8 fps avg) |

Frust pass p50 (ms): rebuild 0.11, layout 0.01, paint 0.06, encode 0.02, acquire 4.23, submit 2.16; layout_us>0 on 18,068/18,177 frames; GPU (gpu_q=1 on 18,137) total p50/p95 0.29/0.30 ms

**Flutter wins S4 on the work axis** — 1.34 vs 2.37 ms p50, 1.52 vs 2.57 p95 — with neither app missing the 60 Hz budget. Frust's row fell 3.69 → 2.37 (−35.6 %) with the GPU pass at 0.29 ms (0.62 before); the plan's 2.0 ms bar is missed by 0.37 ms. The † total's median reads 6.64 (16.60 before) because the acquire wait is bimodal this pass — p50 4.23 ms, p90 14.68, against 12.70 / 13.35 in September — at the same ~60.6 fps: a CADisplayLink phase change, not a cost change, and the work row is unaffected by it.

### S5

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 17.07 | 17.98 | 18.34 | 38.87 | 12,446 (68.47%) | 17,709 (97.43%) | 18,176 (~60.6 fps avg) |
| Frust CPU work (total − acquire wait) | 5.50 | 6.21 | 6.45 | 10.22 | 0 (0.00%) | 18 (0.10%) | 18,176 (~60.6 fps avg) |
| Flutter (profile) | 2.73 | 3.65 | 3.93 | 5.46 | 0 (0.00%) | 0 (0.00%) | 18,176 (~60.6 fps avg) |

Frust pass p50 (ms): rebuild 0.56, layout 0.01, paint 0.06, encode 0.01, acquire 11.61, submit 4.87; layout_us>0 on 18,176/18,176 frames; GPU (gpu_q=1 on 18,136) total p50/p95 1.36/1.38 ms

**Not a like-for-like row on this device.** Frust's work row is 5.50 ms (5.47 before — flat; the plan's 3.0 ms bar is missed by 2.50): its GPU pass fell 4.64 → 1.36 ms while `submit` stayed 4.89 → 4.87, so the cost sits on the CPU side of submit (the column's image uploads), and the full column is decoded on the capture. Flutter's 2.73 ms (2.62 before) is mostly placeholder paint: its cells decode on creation through `decodeImageFromPixels` and the A13 does not keep up with the 10 s sweep (deviation 1), so the Flutter figure is the cost of a column that is not drawing its images. This build emits no `frust-perf img` counter line (deviation 7); the content gate on the Frust side rests on the capture.

### S6

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 17.39 | 17.90 | 19.40 | 24.61 | 17,246 (94.76%) | 18,094 (99.42%) | 18,199 (~60.7 fps avg) |
| Frust CPU work (total − acquire wait) | 3.87 | 4.22 | 5.86 | 14.63 | 0 (0.00%) | 9 (0.05%) | 18,199 (~60.7 fps avg) |
| Flutter (profile) | 4.33 | 5.07 | 5.45 | 16.26 | 0 (0.00%) | 18 (0.10%) | 18,221 (~60.7 fps avg) |

Frust pass p50 (ms): rebuild 0.09, layout 0.23, paint 0.51, encode 0.20, acquire 13.49, submit 2.83; layout_us>0 on 18,199/18,199 frames; GPU (gpu_q=1 on 18,159) total p50/p95 0.46/0.48 ms

**Frust takes the S6 work axis now** — 3.87 vs 4.33 ms p50, 4.22 vs 5.07 p95 (5.57 vs 4.30 in September) — with neither app missing the budget. The resident glyph-run cache took the S6 encode window from 3.99 to 1.57 ms and the per-glyph encode from 4.54 to 1.005 µs, with the GPU pass at 0.46 ms (1.64 before); the plan's 4.3 ms work-row bar is met at 3.87, its 1.5 ms encode and 1.0 µs per-glyph bars are missed by 0.065 ms and 0.005 µs (criteria block).

### S3

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | 23.97 / 29.73 / 33.03 / 33.05 | 180 | **not captured** | 0 |
| create 10k | 16.15 / 22.62 / 23.09 / 23.18 | 184 | **not captured** | 0 |
| update every 10th of 10k | 19.21 / 22.89 / 23.82 / 23.97 | 180 | **not captured** | 0 |
| swap | 18.78 / 22.05 / 24.32 / 25.24 | 180 | **not captured** | 0 |
| clear | 19.44 / 22.54 / 22.91 / 23.01 | 180 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 13.04 | 17.36 | 22.47 | 39.40 | 7,604 (41.83%) | 10,114 (55.63%) | 18,180 (~60.6 fps avg) |
| Flutter (profile) | 5.08 | 12.81 | 16.09 | 18.26 | 4 (0.44%) | 273 (30.37%) | 899 (~3.0 fps avg) |

Cycle health — Frust (profile) `s3-create1k` reopens per kept run: 19–19; Flutter (profile) `s3-create1k` reopens per kept run: 5–20.

**All five Frust reconcile ops are captured, faster or flat on the † totals; the UI half got heavier under the acquire wait.** Overall series p50 16.73 → 13.04 ms (−22 %), p95 18.48 → 17.36, 60 Hz misses 55.1 → 41.8 %; per op `create 1k` 26.41 → 23.97, `create 10k` 24.32 → 16.15, `update` 20.83 → 19.21, `swap` 19.89 → 18.78, `clear` 19.17 → 19.44. Inside the op frames (the `stats.py` `[start, end)` windows) the render half got cheaper on every op (submit 2.33–4.45 → 1.69–2.45 ms) while rebuild rose +44…+48 % on `clear`, `create 10k`, `swap` and `update` (4.18 → 6.02, 8.93 → 13.06, 2.98 → 4.42, 3.84 → 5.58 ms) and the CADisplayLink wait absorbed the difference — the same four-op rebuild move the Xiaomi 12 shows at ~2× on the same build and the OnePlus 9 on `87271063` does not (deviation 2; on the Xiaomi it is scheduler placement of the UI thread, not reconcile cost — its S3 prose — and the iPhone's is not scheduler placement but the build — the September-tree A/B below); `create 1k`'s rebuild is flat (1.71 → 1.70) with submit 10.74 → 11.95. Flutter lands no per-op frame at all on this device (its `create 1k` reopens 5–20 per run against Frust's 19; deviation 8).

### S7

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (FrontBoard bootstrap → scene content ready, median of kept launches) | ~183 ms (173/183/250; first of 4 discarded) | ~171 ms (171/172/161; first of 4 discarded) |
| Framework-reported first-frame span | ~201 ms median (`first_frame_presented`, 95–304 across 12 launches; `adapter_ready` median 84 ms) | ~9 ms median (`first_frame_ms`, 6–49 across 12 launches) |
| Idle CPU over each sampled S7 run's recorded window (Instruments sysmon, Δ time on core / Δt) | ~1.8% avg (max 1.9%, 5 sampled runs, 77 samples) | ~4.0% avg (max 4.1%, 5 sampled runs, 80 samples) |
| Idle memory (phys_footprint, mean over the sampled runs' last sample) | ~38.9 MiB (38.6–39.3, 5 sampled runs) | ~47.7 MiB (47.7–47.8, 5 sampled runs) |

**First external cold start and idle rows on iOS; the framework first frame doubled.** FrontBoard bootstrap → scene-content-ready: Frust 183 ms median (173/183/250 kept; 251 discarded) against Flutter 171 (171/172/161) — Flutter is 12 ms faster on the one instrument that measures both apps the same way, and Frust's launches are bimodal. Idle over the recorded windows: Frust ~1.8 % CPU / 38.9 MiB footprint against Flutter ~4.0 % / 47.7 MiB (five sampled runs each, 77 / 80 samples); this build's S7 page is frame-gate idle (3–6 raw frame lines per 60 s run, where the 2026-09-06 build emitted ~61 per second), so the idle rows read a resting app. The framework-reported first frame is 201 ms median (95–304; 100 and 64–176 in September): `adapter_ready` rose 22 → 84 ms (75–107; 21–34 before) and the font preinit join moved with it — four launches joined at 56–113 ms and eight at 177–273 (six at 51–65 and six at 100–159 before) — with `first_frame_presented` = `font_preinit_joined` + 5–33 ms in eleven of the twelve launches (the twelfth presented 8 ms after `adapter_ready`, at 95). The ≤ 85 ms criterion is missed on every launch; Flutter's own entry span is 9 ms (6–49).

### Startup sub-step split (2026-10-19)

**Method.** The `adapter_ready` move read with the sub-step stamps of PR #97 (`main` `ccd0b47e`; this run's base is `a5f284d7`, which adds only benchmark notes). One signed profile `Runner.app` built from the checkout's own CLI (`frust build ios --profile --define FRUST_TRACE_RAW=1` in `benchmarks/frust_bench`), installed once by `run.sh`, then `run.sh s7 --app frust --platform ios --runs 12 --duration 60`: same iPhone SE (iPhone12,8) and iOS (26.7.1, build 23H30) as 2026-10-17, USB power, Xcode.app quit, no Instruments, no sysmon, no sampler, nothing else on the phone (identifier redacted). All 12 launches carry a startup line; medians and [min–max] are over all 12, as the S7 rows above. The ten added keys are durations (`…_ms`), truncated to whole milliseconds; the existing keys are offsets from `init_entry`. The iOS shell stamps `render_thread_start_ms` (not an `install_rest` carved from it); it is printed after `adapter_ready` but is the part of the gap before the install starts (render-thread spawn and scheduling, derived as the gap less the install's own total).

| Key (ms) | today, 12 launches, median [min–max] |
|---|---|
| `font_preinit_joined` | 150.5 [55–344] |
| `adapter_ready` | 82 [77–151] |
| `first_rebuild_done` | 163.5 [83–380] |
| `first_encode_done` | 163.5 [83–380] |
| `first_frame_presented` | 169 [89–397] |
| **gap** `adapter_ready − init_entry` | 82 [77–151] |
| `engine_surface_ms` | 9.5 [8–65] |
| — of which `device_create_ms` | 9 [8–60] |
| `cache_seed_ms` | 0 [0–0] |
| `caps_probe_ms` | 0 [0–0] |
| **`shader_modules_ms`** | 69 [64–79] |
| `warm_up_start_ms` | 0 [0–0] |
| `engine_rest_ms` | 0 [0–2] |
| `install_rest_ms` | 0 [0–1] |
| sum of the seven install parts | 79.5 [74–140] |
| gap − sum (≈ `render_thread_start_ms` + truncation) | 3 [2–11] |
| `render_thread_start_ms` (inside the gap) | 1 [0–8] |
| `font_system_ms` (preinit thread) | 149.5 [55–341] |
| `font_app_sync_ms` | 0 [0–0] |
| `first_encode_done → first_frame_presented` | 6 [5–17] |

**Verdict — the owning sub-step.** `shader_modules_ms` owns the move: **69 of the 82 ms gap `adapter_ready − init_entry` (84 % median; 83 % on the 11 launches after the first, 48–88 % over all 12)**. It is the only part above 10 ms in the steady launches: `engine_surface_ms` 9.5 (its `device_create_ms` 9 — the Metal device is created on the install path here, unlike Android's pre-created one), `cache_seed_ms`, `caps_probe_ms`, `warm_up_start_ms` all 0, `engine_rest_ms` and `install_rest_ms` 0–2. The three candidates the criteria block named — driver-cache seeding, the capability probe, the warm-up worker spawn — are excluded: they cost nothing measurable. The stamp covers the same twelve `create_shader_module` calls as on Android (`ShaderLibrary::new` plus `EngineShaders::register_family`), ≈5.7 ms a module on the A13. The seven install parts plus `render_thread_start_ms` account for the gap to within 3 ms on all 12 launches (the residue is the truncation of eight whole-millisecond durations). The seven parts alone are within 5 ms on 11 of 12; launch 1, the first launch after the fresh install, spends 8 ms in the render-thread spawn (gap 151, seven parts 140) next to `engine_surface_ms` 65 with `device_create_ms` 60 — first-ever Metal device creation after install — and `font_system_ms` 341. That launch is a warm-up launch the protocol discards (PROTOCOL §4); the verdict holds on all 12.

**The stamped build did not move `adapter_ready`; the first-frame median is outside ±10 % and follows the font join.** `adapter_ready` 82 ms median against 84 on 2026-10-17 (−2 %; [77–151] against [75–107], the high end being launch 1). `first_frame_presented` 169 ms median against 201 (−16 %, outside the ±10 % band; [89–397] against [95–304]). The cause is not the install path: as on 2026-10-17 the first frame waits on `font_preinit_joined`, which here ranges 55–344 ms (sorted: 55, 80, 91, 92, 99, 141, 160, 171, 185, 242, 300, 344), and `font_system_ms` (parley `FontContext::new` on the preinit thread) is 150 ms median [55–341] — within 1–3 ms of the join on every launch, i.e. the join is the font system — so the first-frame median moves with the bimodal/wide font-system time, which this run drew lower than the previous one; `first_encode_done → first_frame_presented` is 6 ms [5–17]. The unaltered install path (`adapter_ready`) is the figure the stamped build must not move, and it did not.

**Phase 2 target.** Same as Android: f2-01's remedy (a) — take the strip-program module creation off the install path (create only the first frame's own variants, the rest on the warm-up worker that already builds their pipelines, or collapse the family into one module with several entry points if the cost is per-module overhead). On the iPhone it is worth ≈69 of 82 ms, and the bar is the September `adapter_ready` of 22 ms. The font system (150 ms median on the preinit thread, deciding `first_frame_presented` here) is a second, separate lever not named by remedies (a)–(d) and is filed as a finding for the conductor, not in this card's scope.

### Device-open fix re-measure (2026-10-20)

**Method.** The f2-01 change (`3658806f`, PR #102: shader modules registered at install and each created on first use, the first frame's four created side by side on a short-lived pool started at install) read on `main` `9a70c1aa` with the sub-step split's recipe above: one signed profile `Runner.app` from `benchmarks/frust_bench` (`frust build ios --profile --define FRUST_TRACE_RAW=1`), installed once by `run.sh`, then `run.sh s7 --app frust --platform ios --runs 12 --duration 60`; same iPhone SE (iPhone12,8) and iOS (26.7.1, build 23H30), USB power, Xcode.app quit, no Instruments, no sampler, nothing else on the phone (identifier redacted). The before column is the sub-step split above. The iOS startup line carries no `first_submit_pipeline_wait` key, so the post-encode guard is read as `first_encode_done → first_frame_presented`.

| Key (ms) | before — `main` `42ef10d9`, 2026-10-19 (the sub-step split above) | after — `main` `9a70c1aa`, today, same recipe |
|---|---|---|
| `font_preinit_joined` | 150.5 [55–344] | 94.5 [52–322] |
| `adapter_ready` | 82 [77–151] | **12 [10–45]** |
| `first_rebuild_done` | 163.5 [83–380] | 103.5 [63–362] |
| `first_encode_done` | 163.5 [83–380] | 103.5 [63–362] |
| `first_frame_presented` | 169 [89–397] | **108.5 [75–377]** |
| **gap** `adapter_ready − init_entry` | 82 [77–151] | 12 [10–45] |
| `engine_surface_ms` | 9.5 [8–65] | 9.5 [8–34] |
| — of which `device_create_ms` | 9 [8–60] | 8.5 [7–29] |
| `cache_seed_ms` | 0 | 0 |
| `caps_probe_ms` | 0 | 0 |
| **`shader_modules_ms`** (now the registration alone) | 69 [64–79] | 1 [0–2] |
| `warm_up_start_ms` (now includes the pool's spawn) | 0 | 0 |
| `engine_rest_ms` | 0 [0–2] | 0 [0–2] |
| `install_rest_ms` | 0 [0–1] | 0 [0–1] |
| sum of the seven install parts | 79.5 [74–140] | 10 [8–39] |
| `render_thread_start_ms` (inside the gap) | 1 [0–8] | 0 [0–5] |
| gap − sum − `render_thread_start_ms` | 2 [1–3] | 2 [0–3] |
| `font_system_ms` (preinit thread) | 149.5 [55–341] | 94 [51–319] |
| `font_app_sync_ms` | 0 | 0 |
| `first_encode_done → first_frame_presented` | 6 [5–17] | **8.5 [4–15]** |

**Verdict against the card's bars.** **`adapter_ready` 12 ms [10–45] against 82 before and September's 22 — the device-open move is gone** (`shader_modules_ms` 69 → 1; the seven install parts plus `render_thread_start_ms` still close the gap to within 3 ms on every launch, so nothing moved into an unstamped place). **The post-encode span is 8.5 ms [4–15], inside the ≤ 25 ms guard** — unlike the OnePlus 9 and the Pixel 5, the A13's first frame does not wait for the pool, because on iOS the first rebuild cannot start before the font preinit join, which lands 39–277 ms after `adapter_ready` and leaves the pool's module creation finished. **`first_frame_presented` 108.5 ms [75–377] against the ≤ 100 ms bar — missed by the font join, not by the install path:** on every launch the first rebuild follows `font_preinit_joined` by 8–40 ms and the frame presents 13–55 ms after the join; the four launches whose join landed at 52–54 ms present at 75–85 ms. The join (94.5 ms [52–322]; `font_system_ms` 94 [51–319]) is the iOS 26.7.1 CoreText enumeration the September-tree block below shows does not follow the build, so, as the card rules, the residue is the font system's and `adapter_ready` is this change's iPhone figure — met. The first-frame median moved 169 → 108.5 ms, but this session's join also drew lower (150.5 → 94.5), so that move is not a measure of the change.

**Metal engine gates, same `main`.** On the Mac's Apple M4 (`WGPU_BACKEND=metal`, `--ignored --test-threads=1`): `frust-gpu` 28 passed, `frust-engine` 81 passed, `engine_goldens` + `alpha_polarity` 5 passed in both sampler arms (default and `FRUST_ENGINE_IMAGE_SAMPLER=0`, `FRUST_GOLDEN_EXPECT_ADAPTER=Apple M4`), no golden changed; on the iOS Simulator (Apple2 adapter): `frust-testing --test ios_sim` 2 passed, the `frust-engine` seam suites 19 passed, `frust-gpu --test shared_encoder` 3 passed. Lazy module creation renders pixel-identically on Metal.

**What is left on the iPhone.** The font system is the only remaining lever for the iPhone's first frame: f2-02 (deferred system fonts) for apps that register fonts. `frust_bench` registers none, so its first frame stays bound to the join.

### Deferred system fonts (f2-02) — NO-GO (2026-10-20)

**Method.** The f2-02 change (branch `perf/ios-deferred-system-fonts` at `19adf5eb`, kept on origin, not merged): on iOS, an app that registered fonts shapes its first frames with a systemless font collection whose generic slots map to the registered families, while the Core Text system collection builds on a background thread and is adopted at a later frame top, re-laying the screen out once (`frust-perf startup-late font_system_adopted=… font_system_ms=…`). Read on huddle — the one app here that registers fonts (`frust_glyph::install()`) — as `main` `9dac101b` against the branch, and on `frust_bench` (no registered fonts) on the branch, with the recipe of the blocks above (`frust build ios --profile --define FRUST_TRACE_RAW=1`; `run.sh s7 --app frust --platform ios --runs 12 --duration 60`, huddle with `--bundle-id it.f0x.huddle`): same iPhone SE and iOS 26.7.1, USB power, Xcode.app quit, no sampler, one screenshot ≈ 8 s into run 1 of each huddle leg (identifier redacted).

| Key (ms), 12 launches, median [min–max] | huddle — `main` `9dac101b` | huddle — branch `19adf5eb` | `frust_bench` — branch | `frust_bench` — `main` (block above) |
|---|---|---|---|---|
| `font_preinit_joined` | 92 [52–369] | 3 [2–4] | 99.5 [53–157] | 94.5 [52–322] |
| `adapter_ready` | 10.5 [9–354] | 15 [10–19] | 12 [10–14] | 12 [10–45] |
| `first_frame_presented` | 112.5 [75–1670] | **42 [36–53]** | 114 [74–172] | 108.5 [75–377] |
| `first_encode_done → first_frame_presented` | 6 [4–1261] | 27 [20–32] | 5.5 [4–13] | 8.5 [4–15] |
| `font_system_ms` | 91.5 [52–365] | 192 [82–302] (`startup-late` line) | 98.5 [52–156] | 94 [51–319] |
| `font_system_adopted` (`startup-late` line) | — | 203.5 [84–319] | — (no line) | — |

**Verdict — NO-GO: the first-frame bar is met, the no-visible-swap bar is not.** huddle's first frame falls 112.5 → **42 ms** against the ≤ 85 ms bar, the join 92 → 3 ms, and `frust_bench` is unchanged (114 against 108.5, join 99.5 against 94.5, no `startup-late` line). But the card also requires the first frames and the adopted ones to draw the same text, and they do not:
1. **The systemless window draws huddle's body text in a different face.** huddle's channel names, descriptions and section headers end in the system sans; with no system fonts, those styles fall to the generic mapping and draw in a Glyph registered face with different metrics (a diagnostic build that never adopts, not committed, shows taller rows and the first direct message pushed below the fold). Adoption at ≈ 200 ms (84–319) swaps the whole screen to the system font and re-lays it out — a visible font swap on every launch. The deferral is swap-free only for an app whose entire first screen resolves to faces it registered; no iOS app in this repository is one.
2. **One cached layout stays in the window's face after adoption.** The app bar's "HQ" tile draws in the system sans on `main` and in the Glyph face on the branch 8 s in: a widget-cached `TextLayout` keyed on text, style and width is not re-shaped when the collection under it changes (the Glyph avatar caches the same way). The settled screenshots otherwise match, the clock aside.

**Side reading.** With the join off the path, huddle's post-encode span is 27 ms [20–32] against f2-01's ≤ 25 ms guard — the first frame now waits for f2-01's module pool, as on the OnePlus 9 and the Pixel 5 above; with the join on the path (`main`, `frust_bench`) the pool is finished first and the span is 5.5–8.5 ms. The background build itself slows from ≈ 92 to ≈ 192 ms when it shares the CPU with the first frames.

**What stays.** The iPhone's first frame remains bound to the font preinit join (`frust_bench` 108.5, huddle 112.5 ms median) — recorded in `docs/LIMITATIONS.md`'s `shell-ios-first-frame-above-goal`.

### S8

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (profile) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 18 | 115 | n/a |
| write i64 | 21 | 65 | n/a |
| write f64 | 24 | 54 | n/a |
| write String | 21 | 55 | n/a |
| write Vec\<String\> | 28 | 61 | n/a |
| read (unique key, forces channel) bool | 4 | 2697† | ~0 |
| read (unique key, forces channel) i64 | 3 | 2697† | ~0 |
| read (unique key, forces channel) f64 | 4 | 2697† | ~0 |
| read (unique key, forces channel) String | 4 | 2697† | ~0 |
| read (unique key, forces channel) Vec\<String\> | 6 | 2697† | ~0 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust still wins S8 by a wide margin — and every Frust row is 3–6× its September cost.** Writes 18–28 vs 54–115 µs/call, a per-key read 3–6 µs against the whole-store `reload()` at 2,697 µs, zero `s8-errors`; but Frust's writes were 3–5 µs and its reads 1 µs on 2026-09-06, while the Android S8 rows are flat on the same build (Xiaomi 12 writes −10…+4 %). An A/B taken right after the matrix (three 30 s runs each with Xcode.app open and with it quit) reads the same on both legs — writes 15–29 µs, reads 3–6 — so the Instruments rig is not the cause; what differs from September is the build (`21076221` → `20e0aa2e`) and iOS (26.6.1 → 26.7.1), which this pass does not separate (deviation 3). The block graded `ok` at the first attempt (2,002 lines per run; September's stale `DEGENERATE` check is gone).

### vs 2026-09-06 (same device, same scenarios)

Frust CPU-work row (total − acquire), Δ positive = slower / worse:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|---|---|
| S1 | 5.79→5.57 | -3.7% | 6.47→6.70 | +3.4% | 6.90→7.09 | 10.62→14.35 | 0.00%→0.00% | n/a→n/a |
| S2 | 6.47→5.96 | -7.8% | 7.50→7.03 | -6.3% | 7.99→9.26 | 11.06→18.95 | 0.00%→0.01% | n/a→n/a |
| S4 | 3.69→2.37 | -35.6% | 4.39→2.57 | -41.4% | 4.60→2.71 | 8.83→10.13 | 0.00%→0.00% | n/a→n/a |
| S5 | 5.47→5.50 | +0.6% | 6.11→6.21 | +1.8% | 6.42→6.45 | 10.31→10.22 | 0.00%→0.00% | n/a→n/a |
| S6 | 5.57→3.87 | -30.5% | 6.28→4.22 | -32.7% | 6.64→5.86 | 13.80→14.63 | 0.00%→0.00% | n/a→n/a |
| S3 (overall series) | 16.73→13.04 | -22.0% | 18.48→17.36 | -6.1% | 24.69→22.47 | 31.79→39.40 | 55.11%→41.83% | n/a→n/a |

Frust S3 per-op (reconcile frames):

| Op | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | n frames |
|---|---|---|---|---|---|
| s3-clear | 19.17→19.44 | +1.4% | 19.94→22.54 | +13.0% | 180→180 |
| s3-create10k | 24.32→16.15 | -33.6% | 25.05→22.62 | -9.7% | 190→184 |
| s3-create1k | 26.41→23.97 | -9.2% | 27.31→29.73 | +8.9% | 180→180 |
| s3-swap | 19.89→18.78 | -5.6% | 21.82→22.05 | +1.0% | 180→180 |
| s3-update | 20.83→19.21 | -7.7% | 21.63→22.89 | +5.9% | 185→180 |

Frust S7 (the external cold start and the idle rows are first captured in this pass and have no September side):

| Metric | old | new | Δ |
|---|---|---|---|
| first-frame span median (ms) | 100 | 201 | +101.0% |

Frust S8 per-op medians (µs/call):

| op:type | old | new | Δ |
|---|---|---|---|
| read:bool | 1 | 4 | +300.0% |
| read:f64 | 1 | 4 | +300.0% |
| read:i64 | 1 | 3 | +200.0% |
| read:string | 1 | 4 | +300.0% |
| read:string_list | 1 | 6 | +500.0% |
| write:bool | 4 | 18 | +350.0% |
| write:f64 | 4 | 24 | +500.0% |
| write:i64 | 3 | 21 | +600.0% |
| write:string | 4 | 21 | +425.0% |
| write:string_list | 5 | 28 | +460.0% |

Flutter control on the same session:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
|---|---|---|---|---|---|---|---|---|
| S1 | 5.10→5.05 | -0.9% | 5.53→5.49 | -0.6% | 5.68→5.68 | 20.78→20.99 | 0.03%→0.04% | 67.0→60.5 |
| S2 | 4.93→4.93 | -0.1% | 5.65→5.61 | -0.7% | 5.97→5.90 | 10.55→10.86 | 0.00%→0.00% | 21.4→21.4 |
| S4 | 1.38→1.34 | -3.4% | 1.61→1.52 | -5.6% | 1.73→1.65 | 7.76→7.53 | 0.00%→0.00% | 67.8→60.8 |
| S5 | 2.62→2.73 | +4.3% | 3.60→3.65 | +1.3% | 3.74→3.93 | 6.60→5.46 | 0.00%→0.00% | 67.4→60.6 |
| S6 | 4.30→4.33 | +0.6% | 5.14→5.07 | -1.5% | 5.38→5.45 | 15.75→16.26 | 0.00%→0.00% | 68.3→60.7 |
| S3 (overall series) | 5.05→5.08 | +0.6% | 12.71→12.81 | +0.8% | 13.54→16.09 | 16.86→18.26 | 0.29%→0.44% | n/a→n/a |

The Flutter control moved −3.4…+4.3 % on p50 across the frame scenarios (its S7 entry span 8 → 9 ms, S8 −13…+16 %), so the session is comparable across an iOS, Xcode and transport change (deviation 5) and the Frust moves are real: S4 −35.6 %, S6 −30.5 %, S3 overall −22.0 %, S2 −7.8 % and S1 −3.7 % on the work row, S5 flat (+0.6 %). The regressions this pass records against September are the S7 first-frame span (+101 %, `adapter_ready` 22 → 84 ms and the font preinit join behind it), the S8 plugin rows (3–6×), and the rebuild half of four S3 ops (+44…+48 %, hidden in the † totals).

### September tree rebuilt on iOS 26.7.1 (2026-10-19)

The method that settled the Xiaomi 12 S3 question, run on the iPhone SE: the 2026-09-07 `main` merge `36ef20db` (a `--profile` build with `FRUST_TRACE_RAW=1`, built by **today's `target/release/frust` CLI** from that tree's own `benchmarks/frust_bench` — the generated project built without falling back to the tree's own CLI; the tree's crates are path dependencies, so the framework code is September's) installed beside `main` `a3d67a1a` built the same way, on the same phone the same day: iOS 26.7.1 (23H30) re-confirmed, Xcode.app quit, no Instruments, USB, Developer Mode. Both `Runner.app`s carry bundle id `it.f0x.frustbench`; the September app's identity is shown by its logs: `frust-perf render-path engine-direct` and `atlas tier=mobile … adapter=Apple A13 GPU` lines (this tree has no separate `frust-render tier=engine` line), `frust-perf raw` lines at 1,718–1,821 per 30 s S3 run (~60.6 fps), the startup line on S7. Order of the legs: September S3/S8/S7, `main` S3/S8, September S3/S8 again, `main` S3/S8 again, then `main` S7. Every leg is 12 complete runs (S3/S8 30 s, S7 60 s), first 2 discarded (PROTOCOL §4); the harness's own `summarize.py` produced the S8 and S7 figures and the S3 op windows are the same half-open `[start, end)` marker windows split by pass (split helper kept in the scratchpad). Columns: **Sep-06** = the 2026-09-06 series (`21076221`, iOS 26.6.1; its published figures), **Sept tree today** = `36ef20db` on iOS 26.7.1 (two legs), **Oct-17** = `20e0aa2e` 2026-10-17 on iOS 26.7.1 (this section's matrix, recomputed from `raw/iphone_se/frust_profile/`), **main today** = `a3d67a1a` (two legs).

**S3 op rebuild p50 over the op frames (ms):**

| Op | Sep-06 (`21076221`, 26.6.1) | Sept tree today (`36ef20db`, 26.7.1) | Oct-17 (`20e0aa2e`) | main today (`a3d67a1a`) |
|---|---|---|---|---|
| clear | 4.18 | 4.27 / 4.24 | 6.02 | 5.98 / 6.02 |
| create 1k | 1.71 | 1.74 / 1.69 | 1.70 | 1.68 / 1.67 |
| create 10k | 8.93 | 9.26 / 9.15 | 13.06 | 12.94 / 12.89 |
| update | 3.84 | 4.00 / 3.89 | 5.58 | 5.54 / 5.58 |
| swap | 2.98 | 2.98 / 3.00 | 4.42 | 4.38 / 4.42 |

**S3 — the build.** The September tree on today's OS keeps September's rebuild (4.2–4.3, 9.2–9.3, 3.9–4.0, 3.0 ms) and `main` the same day reproduces 2026-10-17's (+38…+47 %: 6.0, 12.9, 5.5, 4.4); `create 1k` is flat everywhere (1.7). The submit half runs the other way (September tree 2.3 / 3.3 / 4.0 / 4.3 ms against `main`'s 1.9 / 1.6 / 2.3 / 2.3 on `clear` / `create 10k` / `update` / `swap`; `create 1k` 10.6 vs 11.9), so the move is a rebuild-for-submit trade inside the op frames, unlike the Xiaomi's core-placement move. The † per-op totals are not a stable axis here: they ride the CADisplayLink acquire wait, which flips between sessions (`main` leg 1 `clear` 8.55 ms, leg 2 21.99 ms; the September tree 6.9 / 7.0), so the rebuild and submit columns are the comparison; the overall-series p50 reads 9.19 / 9.54 (September tree) against 9.19 / 16.80 (`main`).

**S8 per-op medians, µs/call:**

| Op | Sep-06 | Sept tree today | Oct-17 | main today |
|---|---|---|---|---|
| write bool | 4 | 3 / 4 | 18 | 19 / 19 |
| write i64 | 3 | 3 / 3 | 21 | 20 / 20 |
| write f64 | 4 | 4 / 4 | 24 | 24 / 24 |
| write String | 4 | 4 / 4 | 21 | 21 / 21 |
| write Vec\<String\> | 5 | 6 / 6 | 28 | 28 / 28 |
| read bool | 1 | 1 / 1 | 4 | 4 / 4 |
| read i64 | 1 | 1 / 1 | 3 | 4 / 4 |
| read f64 | 1 | 1 / 1 | 4 | 4 / 4 |
| read String | 1 | 1 / 1 | 4 | 4 / 4 |
| read Vec\<String\> | 1 | 1 / 1 | 6 | 6 / 6 |

**S8 — the build.** Same phone, same iOS 26.7.1, same day: the September tree 3–6 µs writes and 1 µs reads (all four legs' `s8-errors` 0, 2,002 lines per run), `main` 19–28 µs and 4–6. A 4–6× move that follows the binary, not the OS.

**S7 startup, medians over the 12 launches (ms; min–max):**

| Key | Sep-06 | Sept tree today | Oct-17 | main today |
|---|---|---|---|---|
| `font_preinit_joined` | six at 51–65, six at 100–159 | 164 (52–281) | 185.5 (56–273) | 107 (52–181) |
| `adapter_ready` | 22 (21–34) | 23.5 (21–30) | 84.5 (75–107) | 78.5 (76–83) |
| `first_frame_presented` | 100 (64–176) | 179 (72–302) | 201 (95–304) | 121 (84–197) |

`frust_bench` registers no fonts, so its `font_preinit_joined` is the full CoreText enumeration — the OS half of the first frame only. **S7 — `adapter_ready` is the build; the font join is not.** `adapter_ready` is 21–30 ms on the September tree and 76–83 on `main` the same day, on the same OS (+55 ms, no overlap between the distributions). The join is wide on every tree and session — the September tree reads 52–281, `main` 52–181 the same day, `20e0aa2e` 56–273 — with the same fast (52–59), middle (≈105–115) and slow (≈150–280) modes on both, so it does not follow the build; its between-session spread (median 107 against 164 for two trees one hour apart) is larger than anything the build adds, which is why `first_frame_presented` ranks the wrong way today (September tree 179 vs `main` 121): the join dominates it, `first_frame_presented` = join + 13–24 ms on the September tree and + 13–37 on `main`. The first-frame criterion therefore reads as two separate items: the +55 ms device-open (`adapter_ready`) is the build, the font-preinit time is OS/CoreText state and bimodal-to-trimodal regardless of the build.

**Verdicts (iPhone SE, iOS 26.7.1 23H30, 2026-10-19):** **S3 rebuild: build** (September tree 4.3 / 9.2 / 4.0 / 3.0 ms vs `main` 6.0 / 12.9 / 5.5 / 4.4 on `clear` / `create 10k` / `update` / `swap`; bisect range `c705922b..57c08eb1`). **S8 plugin rows: build** (3–6 vs 19–28 µs writes, 1 vs 4–6 reads). **S7: `adapter_ready` build** (23.5 vs 78.5 ms); **font join OS/session** (September tree 52–281 ms, `main` 52–181, indistinguishable modes). The bisect range for the two build items is `c705922b..57c08eb1` (the `main` merges 2026-10-03 → 10-14), as on the Xiaomi's `adapter_ready` item.

### Methodology deviations (this device)

1. **Flutter's S5 column is placeholder-dominated on this device — the content gate fails on the Flutter side and the S5 rows are not a like-for-like comparison.** The driver's capture (8 s into run 1) shows one decoded cell and two grey placeholders; a 45 s re-check on the same build after the matrix (four screenshots at 8/20/32/45 s, the app launched on its own) never showed more than one decoded cell. The Flutter scenario decodes each cell on creation (`decodeImageFromPixels` per `_ImageCell`) and the A13 does not keep up with the 10 s sweep, so most painted cells are the `ColoredBox` placeholder. The block graded `ok` (its line counts are complete) and the driver did not retry; a retry would reproduce the app's decode pace, not fix it. The 2026-09-06 pass had no iOS capture, so the condition may have been present then too. Frust's column is fully decoded on its capture.
2. **The Frust build is `20e0aa2e`, not the `87271063` of the OnePlus 9 / Pixel 5 pass** — the Xiaomi 12 section's deviation 2 applies verbatim: engine, render, GPU, shell and widget crates identical, `frust-core` with the readability refactor (PR #83). The S3 rebuild move (+44…+48 % on four ops here, ~2× on the Xiaomi 12, absent on the OnePlus 9 at `87271063`) was A/B'd on the Xiaomi after the pass: the refactor is excluded there and the move is UI-thread core placement (its S3 prose). The A13's is **not** that class: the September tree rebuilt on iOS 26.7.1 on 2026-10-19 holds the September rebuild medians (`create 10k` 9.2 vs `main`'s 12.9 ms the same day), so the iPhone's move is a build change in the `c705922b..57c08eb1` range, not the OS and not the readability refactor's pass-through (September-tree block below).
3. **S8 plugin rows 3–6× September's, on iOS only.** Frust's preferences writes read 18–28 µs/call (3–5 on 2026-09-06) and its reads 3–6 µs (1), while the Android S8 rows on the same build are flat (Xiaomi 12 writes −10…+4 %) and Flutter's iOS rows moved −13…+16 %. An A/B after the matrix with Xcode.app open and then quit (three 30 s runs each) reads the same on both legs — writes 15–29 µs, reads 3–6 µs — so the Instruments rig precondition is not the cause. The build (`21076221` → `20e0aa2e`) and the OS (iOS 26.6.1 → 26.7.1) both changed in this pass and were not separated here; the 2026-10-19 September-tree A/B separates them: the September tree (`36ef20db`) rebuilt today reads writes 3–6 µs / reads 1 µs on iOS 26.7.1, `main` the same day 19–28 / 4–6 — **the build, not the OS**, bisect range `c705922b..57c08eb1`. Frust still leads Flutter's channel-crossing rows by 2–5×.
4. **Xcode.app was open on the Mac for the whole run** (Instruments cannot see the phone otherwise), and Instruments sampled every other kept S7 run (3/5/7/9/11) over its last 15 s — five recordings per app of 15–16 samples, none cut short; the saves stretched the S7 blocks to 840 / 890 s. The 2026-09-06 run had neither.
5. **Environment changes since September:** iOS 26.6.1 → 26.7.1 (23H30), Xcode 26.2 → 27.0, a wired USB `devicectl` transport instead of network pairing, and Flutter's eight per-scenario apps rebuilt under Xcode 27 (24.3 MiB each; the September artifacts no longer existed). The Flutter control moved −3.4…+4.3 % on p50.
6. **Flutter's fps column is a presented rate this time** — 18,165–18,253 frames per 300 s (~60.5–60.8 fps) on the 60 Hz panel, where the September build reported 20,092–20,498 (~67–68): the Xcode 27 build's timing callbacks arrive one per present. Compare the two apps on the per-frame rows regardless.
7. **No `frust-perf img` counter line** on this build (as on the Android devices this pass), so the September `skipped`/`evicted`/`resident` figures are not available; the Frust S5 content gate rests on the capture.
8. **S3 Flutter per-op capture:** 0 frames on every op (the known iOS `addTimingsCallback` delivery gap; 26–97 raw lines per run, cycle reopens 5–20) — no cross-app per-op comparison; the block graded `ok` without a retry.
9. **Flutter's S2 script runs once per launch** while Frust's loops it (a declared asymmetry in both scenario sources, unchanged since 2026-09-05); the S2 rows compare per-frame cost on the frames each app painted.
10. **Uncontrolled on iOS:** brightness, airplane mode (on throughout), Wi-Fi (on) and Bluetooth; the charger is USB throughout. Battery level and temperature are now recorded per block (100 %, 30.5–31.9 °C).
11. As on prior passes: Flutter release-mode in-app cross-check not captured; S4 parse wall time not recoverable; S8 burst-during-animation variant not run.

---

## Device: Xiaomi 12 (cupid, Snapdragon 8 Gen 1 / Adreno 730) — headline-tier Android

**Status:** run 2026-10-17 17:08–19:07 UTC, **12 runs × 30 s per scenario per app (S7: 60 s runs), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** **Frust = PROFILE build of `20e0aa2e`** (`main` after the OnePlus 9 / Pixel 5 pass: the engine, render, GPU, Android-shell, shell-common and widget crates are byte-identical to that pass's `87271063`; `frust-core` carries the readability refactor merged since (PR #83, 49 files) — see deviation 2. Built on this Mac with the checkout's own CLI: `frust build apk --profile --target-platform android-arm64 --define FRUST_TRACE_RAW=1`, raw format v4), **Flutter = profile build** (`flutter build apk --profile`, Flutter 3.47.2, rebuilt on this Mac). Raw series under `raw/xiaomi12/frust_profile/<sN>/` and `raw/xiaomi12/flutter/<sN>/` (run-NN.log + run-NN.pss_before/after.txt, stats.txt; S7 adds cpuinfo.txt/coldstart.txt). One unattended `matrix.sh` session from the Mac, all 16 blocks captured and graded `ok` on attempt 1 — no retries. **This pass supersedes the 2026-09-06 pass on this device** (its section: `git show 20e0aa2e:benchmarks/RESULTS.md`).

- Chipset: Snapdragon 8 Gen 1 (SM8450, board `taro`) / Adreno 730, GLES driver V@0615.95. Model: Xiaomi 12 2201123G (serial redacted).
- OS: LineageOS 23.2 — Android 16, build `BP4A.251205.006`, SDK 36 (same build id as on 2026-09-06, but the nightly is `23.2-20261001` and the vendor
  firmware moved since — see the S3 A/B); the fingerprint now reports the stock `cupid_global` Android 15 `OS3.0.4.0` build (was `OS3.0.3.0`). Not MIUI/HyperOS.
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

**Frust wins S3 on every comparable axis; every one of its five reconcile ops is slower than in September.** Overall series p50 4.11 vs 14.63 ms at ~113.5 vs ~3.1 fps, and the five Frust ops all land 340–350 frames per kept series (Flutter's `create 1k` lands 1; deviation 6). Against 2026-09-06 the per-op medians moved `create 1k` 12.37 → 22.46 ms (+82 %), `create 10k` 12.93 → 20.52 (+59 %), `clear` 10.34 → 16.66 (+61 %), `update` 10.73 → 15.22 (+42 %), `swap` 10.22 → 13.91 (+36 %); the overall series p95 rose 8.59 → 12.16 and its 60 Hz miss rate 0.49 → 2.53 %, while the overall p50 fell (4.83 → 4.11). The pass split of the op frames (the same `[start, end)` windows `stats.py` publishes) says where: on `clear`, `create 10k`, `update` and `swap` the whole move is the UI half — **rebuild median 7.08 → 13.38, 7.64 → 16.21, 5.67 → 11.12 and 4.89 → 10.04 ms** — with the render half cheaper (submit 2.97–4.77 → 2.52–2.95 ms); on `create 1k` rebuild is flat (1.90 → 2.57) and **submit doubles (9.12 → 18.23 ms)**, the same shape as the OnePlus 9's `create 1k` on `87271063` (submit 17.98). The four-op rebuild move is not this device's alone: the iPhone SE on the same build moved the same four ops' rebuild +44…+48 % (4.18 → 6.02, 8.93 → 13.06, 2.98 → 4.42, 3.84 → 5.58 ms, hidden in its † totals by the acquire wait), while the OnePlus 9 on `87271063` read `clear` 15.18 → 15.33; it leaves the Xiaomi's reconcile medians where the OnePlus 9's are (13.3–21.4 ms) instead of 3–8 ms under them as in September. The build differs from `87271063` only in `frust-core` (deviation 2) and that was the first suspect; the A/B below clears it. The criterion's recorded series still misses, but the cause is where the phone runs the UI thread, not what the framework does per widget.

**S3 rebuild A/B (2026-10-18; Frust only, the S3 block with the matrix prep, 8–12 runs with the first 2 discarded; `create 10k` rebuild p50 over the op frames, ms; the raw series stay out of the tree):**

| Build / condition | `create 10k` rebuild | UI thread's cores (2 s `/proc` samples) |
|---|---|---|
| 2026-09-06 series (`21076221`, vendor firmware `OS3.0.3.0`) | 7.64 | — |
| `21076221` and `36ef20db` (2026-09-07 `main`) rebuilt today, unperturbed, 9 legs | 16.1–16.6 | silver 0–3 |
| the same `36ef20db` APK with an `adb shell` `/proc` poller every 2 s, 3 legs | 7.6–8.5 | gold 4–5 ~45 % |
| `87271063` (before PR #83) / `20e0aa2e` (after), matrix prep | 16.03 / 15.30 (12.90 in a later session) | silver 0–3 |
| `20e0aa2e` + the poller | 13.7 | silver 0–3 |
| `20e0aa2e` kill switches: `FRUST_NO_THREAD_TUNING` / `FRUST_ENGINE_NO_PARALLEL` / both / `FRUST_NO_FRAME_SYNC` / `FRUST_ADPF=1` | 12.5 / 13.1 / 12.9 / 13.3 / 12.8 | silver 0–3 |
| `20e0aa2e`, `sched_setattr` uclamp.min 512 on the UI thread (scratch build) | refused, `EPERM` | — |
| `20e0aa2e`, UI thread `sched_setaffinity` to cpu 4–7 (scratch build) | **7.96** (p95 8.9 vs 19) | gold 4–6 only |
| every first-parent merge 2026-10-03 → 10-14 (`ec225e7e` … `3770a95f`), 8-run legs | 12.5–16.6 | — |
| `20e0aa2e` with a harmless key event injected every 300 ms (the platform's interaction boost, as real input brings it) | **5.18** (p95 5.5; submit 1.67) | gold/prime 4–7 only |

Reading: the UI thread's total CPU time is the same on the September and the current build (478 vs 512 ticks over a leg); on the September firmware it ran on the gold cores, on today's (`23.2-20261001`, vendor `OS3.0.4.0`; top-app `cpu.uclamp.min` 0, `cpu.uclamp.latency_sensitive` 0) it stays on the silver cluster unless something perturbs the scheduler (the poller's fork bursts do for the September tree, the pin forces it for any tree), and every reconcile burst then runs at 1.79 GHz instead of 2.5–3.0. The bisect this implied is void: both endpoints are slow under identical conditions. ADPF's hint session has no effect on this ROM and a task may not raise its own uclamp, so the mitigation is a platform/thread-policy question (a frame-class affinity or priority policy, or the interaction boost real input brings), not a reconcile fix; the scenario runs without input, so it measures the unboosted state real use rarely sits in during a burst.

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
2. **The Frust build is `20e0aa2e`, not the `87271063` of the OnePlus 9 / Pixel 5 pass.** Between the two, `main` took the frust-core readability refactor (PR #83: `app.rs`/`event.rs` split into directory modules, named dispatch steps, a `ChildFold` for child dispatch flags — 49 files) and the iOS-harness captures (PR #84, harness only); `git diff --stat 87271063 20e0aa2e` is empty for `crates/frust-engine`, `frust-render`, `frust-gpu`, `frust-shell-android`, `frust-shell-common` and `frust-widgets`. Every frame-table and S8 number is therefore a same-engine read; the S3 rebuild move was A/B'd after the pass and the refactor is excluded (S3 prose).
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
