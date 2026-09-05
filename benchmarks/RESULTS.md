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
2 of 12 runs are warm-up and excluded; miss counts are per frame attempted.

> **Results reset 2026-09-05 — engine renderer.** Frust's `vello` renderer was
> deleted on 2026-09-02 and replaced by the frust-owned `frust-engine` strip
> pipeline (PROTOCOL §2.6). Every Frust series below is a `--profile` build of
> `f64be636` on that engine; the 2026-07-21 vello-era matrix (OnePlus 9, iPhone SE,
> Xiaomi 12) was retired in one sweep — its OnePlus 9 and iPhone SE raws are
> gone from `raw/` after the renderer-transition comparison at the bottom of this
> file, its Xiaomi 12 section is kept (condensed) until that device is re-run.
> **Devices in this pass:** OnePlus 9 and iPhone SE (16/16 blocks each). The Pixel 5 was on the rig
> but dropped off USB during its first block and has no series; the Xiaomi 12 was not at desk.
> Toolchain for this pass: Rust 1.98.1 (pinned), Flutter 3.47.2 stable, Xcode 26.2,
> NDK 28.2.13676358, `wgpu` 30.0.1.

## Device: OnePlus 9 (LE2115 "lemonade", Snapdragon 888 / Adreno 660) — mid-tier Android

**Status:** run 2026-09-04/05 (UTC), **12 runs × 30 s per scenario per app (S7: 60 s runs), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** **Frust = PROFILE build of `f64be636`** (engine renderer; `frust build … --profile --define FRUST_TRACE_RAW=1`, raw format v4), **Flutter = profile build** (`flutter build … --profile`, Flutter 3.47.2). Raw series under `raw/oneplus9/frust_profile/<sN>/` and `raw/oneplus9/flutter/<sN>/` (run-NN.log + run-NN.pss_before/after.txt, stats.txt; S7 adds cpuinfo.txt/coldstart.txt). S1–S6 ran in one unattended `matrix.sh` session, S7–S8 in a second session after a driver hang (deviation 1).

- Chipset: Snapdragon 888 (board `lahaina`) / Adreno 660, GLES driver V@0530.53. Model: OnePlus 9 LE2115 (serial redacted).
- OS: **LineageOS 22.2 — Android 15, `lineage_lemonade-userdebug 15 BP1A.250405.007` (nightly 20250414), not the stock OxygenOS of the earlier section**; fingerprint still reports `OnePlus9:14/UKQ1.230924.001`.
- Display: 1080×2400 @ 450 dpi; modes id 0 = 60 Hz (active at idle), id 1 = 120 Hz. **Both `min_refresh_rate`/`peak_refresh_rate` pinned to 120 for every block** (`--pin-refresh 120`, read-back `peak=120.0 min=120.0`, restored to `Infinity`/unset afterwards); Frust's in-app FPS readout showed 121.0 in S1. Both budgets (16.67 / 8.33 ms) reported.
- High-refresh opt-in engaged on both apps (§6): Frust `Surface.setFrameRate`, Flutter `flutter_displaymode` ^0.7.0. Achieved: Frust ~120 fps in every frame scenario; Flutter ~97 (S1), ~108 (S5), ~119 (S4/S6) fps; S2/S3 Flutter paint is event-driven.
- Brightness fixed 128/255, auto-brightness off (`device_state.sh`); airplane on (read-back 1), Wi-Fi off, Bluetooth off, battery saver off, screen timeout 30 min, `svc power stayon true` during blocks; all restored (deviation 8).
- Charger: USB-connected to a 4.5 A charger throughout (adb); `dumpsys battery unplug` spoofed on-battery state before every block. Phone was at a real 1 % at 04:44 local; `--min-level 30` guard waited 6 min (23 → 32 %) before S1; real level 32 → 88 % across S1–S6, 100 % for S7–S8.
- Thermal: gate ceiling 38 °C / 120 s cooldown (`device_state.sh`) before every block. S1–S6 session: start 32.4 °C, peak 36.3 °C, end 33.9 °C; S7–S8 session: start 28.8 °C, peak 28.8 °C, end 27.9 °C. **No cooldown stall in any block** (no `warning: still` in any run.log).
- Toolchain: Rust 1.98.1 (pinned), NDK 27.0.12077973; Flutter 3.47.2 stable / Dart 3.13.2. APKs: frust 34,206,174 B (md5 d11f1aaa…), flutter 27,193,780 B (md5 135e9aab…); both installed fresh (stale installs uninstalled first), md5 re-verified by the driver.
- Visual gate (driver screencap per block): S1/S2/S4/S6 PASS both apps (same deterministic content on both); **S5 geometry PASS — both apps' cells span edge-to-edge with the identical 8 dp side padding**; flutter/s3 capture blank and frust/s5 capture sparse — see deviations 3–4; S7/S8: PASS both apps (S7 pages, S8 quiescent screens).
- Sanitization: S1–S6 staged (`stage_raw.sh`) `RAW_OK`; S7–S8 staged by the driver `RAW_OK`; merged tree re-checked against the same whitelist: `RAW_OK` (192 run logs, 384 PSS snapshots).

### S1

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 8.20 | 9.45 | 10.11 | 47.71 | 35 (0.10%) | 15,188 (42.14%) | 36,038 (~120.1 fps avg) |
| Flutter (profile) | 9.44 | 17.15 | 19.47 | 40.11 | 1,517 (5.19%) | 26,710 (91.46%) | 29,203 (~97.3 fps avg) |

Frust pass p50 (ms): rebuild 0.19, layout 0.00, paint 0.41, encode 0.06, acquire 1.28, submit 6.25; layout_us>0 on 55/36,038 frames; GPU (gpu_q=1 on 35,998) total p50/p95 6.41/6.65 ms


**Frust wins S1** — p50 8.20 vs 9.44 ms, p95 9.45 vs 17.15, p99 10.11 vs 19.47, and 0.10 % vs 5.19 % of frames over the 60 Hz budget, at ~120 vs ~97 fps. Flutter's worst frame is lower (40.1 vs 47.7 ms).

### S2

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 8.83 | 11.45 | 13.09 | 49.98 | 36 (0.10%) | 27,672 (76.84%) | 36,011 (~120.0 fps avg) |
| Flutter (profile) | 15.20 | 24.20 | 27.86 | 82.34 | 2,218 (37.97%) | 5,138 (87.96%) | 5,841 (~19.5 fps avg) |

Frust pass p50 (ms): rebuild 0.09, layout 1.84, paint 0.28, encode 0.04, acquire 0.30, submit 5.96; layout_us>0 on 36,011/36,011 frames; GPU (gpu_q=1 on 35,971) total p50/p95 4.79/4.97 ms


**Frust wins S2** — p50 8.83 vs 15.20 ms, p95 11.45 vs 24.20, 60 Hz misses 0.10 % vs 37.97 % of frames attempted. Flutter's lower absolute 8.33 ms miss count reflects its ~6× smaller event-driven frame total (5,841 vs 36,011), not cheaper frames. Frust layout ran on every frame (p50 1.84 ms) — a genuine per-frame relayout.

### S3

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | **not captured** | 0 | 10.82 / 11.37 / 11.37 / 11.37 | 2 |
| create 10k | 20.81 / 21.65 / 21.95 / 22.02 | 256 | **not captured** | 0 |
| update every 10th of 10k | 15.24 / 15.90 / 16.04 / 16.14 | 263 | **not captured** | 0 |
| swap | 14.20 / 15.56 / 16.07 / 16.40 | 235 | **not captured** | 0 |
| clear | 14.77 / 15.90 / 16.05 / 16.35 | 272 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 5.57 | 12.16 | 19.25 | 47.08 | 735 (2.14%) | 2,482 (7.23%) | 34,350 (~114.5 fps avg) |
| Flutter (profile) | 19.16 | 33.65 | 36.07 | 83.64 | 593 (64.39%) | 903 (98.05%) | 921 (~3.1 fps avg) |

Cycle health — Frust (profile) `s3-create1k` reopens per kept run: 35–35; Flutter (profile) `s3-create1k` reopens per kept run: 19–19.


**No cross-app per-op comparison** — Flutter's per-op capture landed 2 frames total (create 1k) and none for the other ops (the known `addTimingsCallback` delivery gap), and Frust's `create 1k` sub-marker landed 0 frames this pass (its other four ops 235–272 frames each; deviation 5). Overall series (not apples-to-apples): Frust 5.57 / 12.16 / 19.25 ms over 34,350 frames (35 cycles per run) vs Flutter 19.16 / 33.65 / 36.07 ms over 921 event-driven frames (19 cycles per run); no overall winner declared, per convention.

### S4

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 4.54 | 6.22 | 8.26 | 64.53 | 24 (0.07%) | 331 (0.92%) | 36,060 (~120.2 fps avg) |
| Flutter (profile) | 4.24 | 4.94 | 5.89 | 74.91 | 1 (0.00%) | 12 (0.03%) | 35,842 (~119.5 fps avg) |

Frust pass p50 (ms): rebuild 0.10, layout 0.00, paint 0.05, encode 0.02, acquire 0.10, submit 4.09; layout_us>0 on 22/36,060 frames; GPU (gpu_q=1 on 36,020) total p50/p95 1.12/1.19 ms


**Roughly a tie, Flutter marginally smoother** — animation-during-parse p50 4.24 vs 4.54 ms, p95 4.94 vs 6.22, p99 5.89 vs 8.26, 120 Hz misses 12 vs 331; both hold a locked ~120 fps. Frust's worst frame is lower (64.5 vs 74.9 ms). Parse wall time not captured (deviation 7).

### S5

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 7.78 | 8.85 | 10.41 | 53.00 | 36 (0.10%) | 7,041 (19.53%) | 36,045 (~120.2 fps avg) |
| Flutter (profile) | 6.54 | 14.21 | 16.35 | 26.03 | 253 (0.78%) | 10,921 (33.70%) | 32,404 (~108.0 fps avg) |

Frust pass p50 (ms): rebuild 1.04, layout 0.05, paint 0.08, encode 0.01, acquire 1.12, submit 5.47; layout_us>0 on 36,044/36,045 frames; GPU (gpu_q=1 on 36,005) total p50/p95 5.07/6.67 ms


**Split — Flutter wins the median, Frust wins the tail**: p50 6.54 vs 7.78 ms for Flutter; p95 8.85 vs 14.21 and p99 10.41 vs 16.35 for Frust, with 0.10 % vs 0.78 % of frames over 60 Hz and 19.5 % vs 33.7 % over 120 Hz, at ~120 vs ~108 fps. Frust's pass breakdown is rebuild-dominated (1.04 ms) with layout 0.05 / paint 0.08 ms per frame; read with deviation 4.

### S6

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 8.19 | 8.90 | 10.50 | 49.64 | 44 (0.12%) | 12,609 (34.97%) | 36,052 (~120.2 fps avg) |
| Flutter (profile) | 8.37 | 9.81 | 10.29 | 91.13 | 2 (0.01%) | 18,371 (51.39%) | 35,747 (~119.2 fps avg) |

Frust pass p50 (ms): rebuild 0.16, layout 0.32, paint 0.46, encode 0.04, acquire 0.96, submit 6.16; layout_us>0 on 36,052/36,052 frames; GPU (gpu_q=1 on 36,012) total p50/p95 1.74/1.81 ms


**Near tie, split by metric** — Frust p50 8.19 vs 8.37 ms and p95 8.90 vs 9.81, with fewer 120 Hz misses (35.0 % vs 51.4 %); Flutter p99 10.29 vs 10.50 and fewer 60 Hz misses (2 vs 44). Worst frame: Frust 49.6 vs Flutter 91.1 ms. Both ~120 fps.

### S7

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | ~137 ms (141/126/137; first of 4 discarded) | ~414 ms (414/425/405; first of 4 discarded) |
| Framework-reported first-frame span | ~105 ms median (`first_frame_presented`, 100–122 across 12 launches; `adapter_ready` median 50 ms) | ~80 ms median (`first_frame_ms`, 5–86 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | ~5.9% avg (max 5.9%, 2 samples) | 0 samples |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | ~104.7 MB (104.6–104.9, 10 kept snapshots) | ~142.4 MB (142.0–142.6, 10 kept snapshots) |

**Frust wins S7** — external cold start ~137 vs ~414 ms (~3×) and idle memory ~104.7 vs ~142.4 MB PSS (26 % lower). The framework-reported first-frame span favours Flutter (~80 vs ~105 ms; Frust's `adapter_ready` median 50 ms is the GPU-init floor). Idle CPU is effectively 0 % on both — the Frust line's 5.9 % (2 samples) is a collision artefact, see deviation 2.

### S8

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (profile) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 101 | 288 | n/a |
| write i64 | 94 | 285 | n/a |
| write f64 | 94 | 305 | n/a |
| write String | 95 | 276 | n/a |
| write Vec\<String\> | 95 | 288 | n/a |
| read (unique key, forces channel) bool | 19 | 4383† | ~0 |
| read (unique key, forces channel) i64 | 19 | 4383† | ~0 |
| read (unique key, forces channel) f64 | 19 | 4383† | ~0 |
| read (unique key, forces channel) String | 19 | 4383† | ~0 |
| read (unique key, forces channel) Vec\<String\> | 19 | 4383† | ~0 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust wins S8 decisively** — writes 94–101 vs 276–305 µs/call (~2.9–3.2×); a per-key Frust read costs 19 µs while Flutter's only channel-crossing read is the whole-store `reload()` at 4,383 µs (its ~0 µs cached read is a Dart-map lookup, not a boundary crossing). Zero `s8-errors` on either app.

### Methodology deviations (this device)

1. **Driver hang → two `matrix.sh` sessions.** Session 1 (22:01–23:25 UTC) completed S1–S6 cleanly, then hung before S7's `BLOCK START`: `run_block`'s `sampler="$(android_cpu_sampler_bg …)"` never returned because the sampler's `( while …; done >>file ) &` subshell kept bash's saved copy of the command-substitution pipe open (`lsof`: driver fd 3 read end ↔ sampler fd 10 write end; reproduced in isolation — an `exec >>file 2>/dev/null` inside the subshell fixes it). The orphaned sampler polled `dumpsys cpuinfo` on the idle launcher until the session was killed at 23:48 (its `frust/s7/cpuinfo.txt` is junk). Session 2 (23:52–00:31 UTC, `--scenarios s7,s8 --skip-install`, same pin/guard, identical setup read-back) ran S7–S8 ~27 min after S6 at a real 100 % / 28.8 °C, and cleared both sampler substitutions without hanging (same bash 5.3.15 — the hang is ordering-dependent, not deterministic).
2. **S7 frust block overlap (operator error).** For ~95 s at the start of session 2's S7 frust block a second driver instance launched by this operator ran the same block before being SIGKILLed (no restore side effects). Only run-01/run-02 (discarded warm-ups) overlapped; kept runs 03–12 began after it was gone and carry the normal marker shape (the `s7`+`s7-idle` double start marker is intrinsic — same in the prior pass). Residue: `frust_profile/s7/cpuinfo.txt` holds 6 of its 31 samples from 23:51:25–23:53:57 (same-second duplicates), and the S7 table's Frust idle-CPU line (5.9 %, 2 samples) is exactly the duplicate 23:52:26 samples taken during the double launch; in the other 29 samples the process was absent from `dumpsys cpuinfo` (0 %) — read Frust idle CPU as ~0 %, like Flutter's.
3. **flutter/s3 block screencap blank** — capture timing (one frame ~8 s after foreground; the cycle clears the table every ~1.6 s). Cycle health confirmed (19 `create1k` / 18 `clear` per run, all 12 runs) and a post-matrix hand launch rendered the table (rows 10001–10020). Not a render failure; no re-run.
4. **frust/s5 scene only partially materialised (health caveat).** The block screencap shows one decoded cell and pure background below (no 0x202020 placeholders); a post-matrix hand launch showed image content on 43 / 51 / 0 / 0 / 68 % of sampled pixels at 3 / 8 / 13 / 18 / 25 s — the stream populates but goes fully empty for stretches of every 20 s scroll cycle. Per-frame layout 0.05 / paint 0.08 ms (vs S2's 1.84 / 0.28) corroborates, and the 2026-09-01 engine-tier S5 baseline on this device has the identical signature, so it is systematic rather than a run artefact. Frust's S5 rows therefore composite less than Flutter's edge-to-edge cells; not re-run (the same APK reproduces it).
5. **S3 per-op capture gaps:** Flutter landed 2 frames (create 1k) and none for the other ops (the known `addTimingsCallback` delivery gap); Frust's `create 1k` sub-marker landed 0 frames this pass (create 10k / update / swap / clear: 235–272 frames each).
6. **Flutter S2 row text wraps and overlaps** neighbouring rows on this 384 dp-wide panel (font_scale 1.0, physical 450 dpi, no display override) — bench-app layout, visual only.
7. As on prior passes: Flutter release-mode in-app cross-check not captured; S4 parse wall time not recoverable (timestamp-free capture); S8 burst-during-animation variant not run.
8. **Device-state restore:** session 2's saved state was session 1's already-set-up state, so its own restore left airplane on, the 120 Hz pin and brightness 128; the operator restored the true original by hand at 00:34 UTC (airplane 0, Wi-Fi on, BT off, `peak_refresh_rate` Infinity, `min_refresh_rate` unset, brightness 1/auto; read-back confirmed).
9. **Charger:** USB-connected to a 4.5 A charger throughout, `dumpsys battery unplug` spoofed before every block; the phone started the night at a real 1 %, the `--min-level 30` guard waited 6 min (23 → 32 %), real level rose 32 → 88 % across S1–S6 and read 100 % for S7–S8.
10. **Raw staging:** S1–S6 staged by `stage_raw.sh` (main loop), S7–S8 by session 2's driver (`RAW_OK`); the merged 192-log / 384-PSS tree re-passed the identical whitelist checks.

---

## Device: iPhone SE (2nd gen) (iPhone12,8 / Apple A13 Bionic) — iOS, 60 Hz budget tier

**Status:** run 2026-09-04 22:13Z → 2026-09-05 00:21Z, **full-form: 12 runs × 30 s per scenario per app (S7 also 30 s — the 60 s length only serves Android's idle window), first 2 discarded (10 kept) — PROTOCOL §4 satisfied.** Frust = **PROFILE** build of `f64be636` (engine renderer; `frust build … --profile --define FRUST_TRACE_RAW=1`, raw format v4); Flutter = profile build (`flutter build … --profile`, Flutter 3.47.2). Raw series under `raw/iphone_se/frust_profile/<sN>/` and `raw/iphone_se/flutter/<sN>/` (192 run logs, 12 per scenario per app; no PSS snapshots on iOS).

- Chipset: Apple A13 Bionic (arm64e); iPhone SE 2nd gen (iPhone12,8, hardware D79AP); serial/UDID redacted.
- OS: iOS 26.6.1 (23G83), Developer Mode on, wired `devicectl` transport.
- Display: 60 Hz panel, single mode. **The 8.33 ms column is N/A on this device** (summarize.py still prints it; it is not discussed).
- Refresh opt-in (§6): no >60 Hz mode exists, so the gate is moot; both apps ran at ~61–62 fps in every frame scenario.
- Brightness / airplane / Wi-Fi / BT: **uncontrolled** (no iOS CLI). Charger: USB-connected throughout (devicectl requirement); battery 83 % charging at launch → 100 % by 23:53Z, then full/not-charging to the end.
- Thermal: no sensor CLI; fixed 60 s inter-block cooldown (`--cooldown 60`); no gate stalls possible.
- Toolchain: Xcode 26.2 (17C52), rustc 1.98.1 (pinned), Flutter 3.47.2 stable.
- Visual gate: **skipped** (no iOS screenshot CLI). Driver marker/line-count sanity passed on all 16 blocks at the first attempt (frust blocks 375–384 s wall, Flutter 407–411 s incl. per-scenario install + container pull).
- Sanitization: `RAW_OK` from both driver sessions; merged tree re-checked — 0 lines outside the whitelist in all 192 logs.
- iOS conventions: Frust `total_us` (rows marked †) folds in the CADisplayLink acquire/vsync wait (`acquire` p50 10.9–13.3 ms), so † totals pin to the 16.67 ms cadence; **the "CPU work" row is `total − acquire` = rebuild+layout+paint+encode+submit** and is the per-frame comparison row. Flutter's `totalSpan` goes negative under load, so its frame total is `build_us+raster_us`. **For the engine, `submit` waits on the GPU, so that row is CPU+GPU on the Frust side while Flutter's `build+raster` is CPU only — the two are not like-for-like, and no per-frame winner is declared from them; Frust's real GPU time is the `gpu_total` figure in each pass-breakdown line (Metal timestamps, `gpu_q=1`).**

### S1 — Animation storm

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 16.61 | 18.19 | 18.56 | 26.35 | 8,480 (46.09%) | 18,278 (99.34%) | 18,400 (~61.3 fps avg) |
| Frust CPU work (total − acquire wait) | 5.79 | 6.48 | 6.92 | 13.04 | 0 (0.00%) | 15 (0.08%) | 18,400 (~61.3 fps avg) |
| Flutter (profile) | 4.91 | 5.56 | 5.71 | 20.82 | 7 (0.04%) | 11 (0.06%) | 18,575 (~61.9 fps avg) |

Frust pass p50 (ms): rebuild 0.03, layout 0.01, paint 0.05, encode 0.05, acquire 10.87, submit 5.62; layout_us>0 on 18,400/18,400 frames; GPU (gpu_q=1 on 18,360) total p50/p95 4.38/4.58 ms

**No per-frame winner is declared on this device** (asymmetric instruments, as on every iOS pass): both apps hold the 60 Hz cadence at ~61–62 fps with zero budget misses on their work rows. Frust's frame is ~0.2 ms of CPU plus 4.4 ms of GPU (Metal timestamps, p50/p95 4.38/4.58 ms) and the `submit` pass waits on that GPU work, so the CPU-work row (5.79 ms) is CPU+GPU; Flutter's 4.91 ms is build+raster CPU time with its GPU time not exposed. Frust's † totals show 46 % misses only because they are the cadence itself.

### S2 — Long-list scroll (10k rows)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 18.81 | 20.76 | 21.44 | 24.99 | 10,845 (59.45%) | 11,570 (63.43%) | 18,241 (~60.8 fps avg) |
| Frust CPU work (total − acquire wait) | 6.34 | 7.33 | 7.73 | 11.64 | 0 (0.00%) | 30 (0.16%) | 18,241 (~60.8 fps avg) |
| Flutter (profile) | 4.83 | 5.52 | 5.84 | 11.07 | 0 (0.00%) | 11 (0.17%) | 6,422 (~21.4 fps avg) |

Frust pass p50 (ms): rebuild 0.09, layout 2.47, paint 0.14, encode 0.03, acquire 12.57, submit 2.97; layout_us>0 on 18,241/18,241 frames; GPU (gpu_q=1 on 18,201) total p50/p95 3.08/3.14 ms

**No per-frame winner declared** (same asymmetry): Frust CPU ≈2.7 ms (relayout on every frame, layout p50 2.47 ms) + GPU 3.1 ms (p50/p95 3.08/3.14) inside a 6.34 ms CPU+GPU-wait row, vs Flutter's 4.83 ms build+raster CPU with GPU unexposed; neither misses the 60 Hz budget on work. Frust paints continuously (18,241 frames); Flutter paints event-driven (6,422 frames, ~642/run).

### S3 — Table ops (continuous cycling)

Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):

| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |
|---|---|---|---|---|
| create 1k | 16.75 / 16.98 / 17.07 / 17.17 | 180 | **not captured** | 0 |
| create 10k | 16.04 / 17.03 / 18.86 / 19.17 | 189 | **not captured** | 0 |
| update every 10th of 10k | 15.68 / 16.95 / 17.59 / 17.65 | 180 | **not captured** | 0 |
| swap | 15.99 / 17.11 / 18.24 / 19.73 | 180 | **not captured** | 0 |
| clear | 15.72 / 16.87 / 17.28 / 18.54 | 180 | **not captured** | 0 |

Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) | 16.79 | 18.78 | 24.89 | 31.71 | 11,389 (62.32%) | 17,288 (94.59%) | 18,276 (~60.9 fps avg) |
| Flutter (profile) | 5.01 | 12.84 | 13.72 | 16.80 | 2 (0.22%) | 268 (29.29%) | 915 (~3.0 fps avg) |

Cycle health — Frust (profile) `s3-create1k` reopens per kept run: 19–19; Flutter (profile) `s3-create1k` reopens per kept run: 5–20.

**No cross-app per-op comparison exists** (Flutter per-op capture landed 0 frames, as on every prior pass). Frust's per-op figures are `total_us`, hence cadence-bound (p50 15.7–16.8 ms), and shifted by one op (deviation 6); no winner is declared from the overall series.

### S4 — Heavy-work responsiveness (JSON parse + concurrent animation)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 16.72 | 16.99 | 17.84 | 23.45 | 12,598 (68.69%) | 18,233 (99.42%) | 18,340 (~61.1 fps avg) |
| Frust CPU work (total − acquire wait) | 3.42 | 4.03 | 4.28 | 15.44 | 0 (0.00%) | 6 (0.03%) | 18,340 (~61.1 fps avg) |
| Flutter (profile) | 1.34 | 1.51 | 1.63 | 6.82 | 0 (0.00%) | 0 (0.00%) | 18,630 (~62.1 fps avg) |

Frust pass p50 (ms): rebuild 0.09, layout 0.01, paint 0.04, encode 0.01, acquire 13.30, submit 3.25; layout_us>0 on 18,340/18,340 frames; GPU (gpu_q=1 on 18,300) total p50/p95 0.67/0.86 ms

**No per-frame winner declared**: both hold a locked ~61–62 fps animation through the parse with zero 60 Hz misses on work. Frust's row is 3.42 ms CPU+GPU-wait with only 0.67 ms of GPU time (p50) — the rest is the synchronous submit/present path — vs Flutter's 1.34 ms build+raster CPU (GPU unexposed). Parse wall time not captured.

### S5 — Image pipeline (decode-and-display while scrolling)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 17.12 | 18.40 | 19.20 | 23.66 | 13,472 (73.69%) | 17,893 (97.88%) | 18,281 (~60.9 fps avg) |
| Frust CPU work (total − acquire wait) | 5.31 | 6.58 | 7.36 | 10.99 | 0 (0.00%) | 34 (0.19%) | 18,281 (~60.9 fps avg) |
| Flutter (profile) | 2.74 | 3.67 | 3.95 | 5.74 | 0 (0.00%) | 0 (0.00%) | 18,598 (~62.0 fps avg) |

Frust pass p50 (ms): rebuild 0.57, layout 0.03, paint 0.04, encode 0.01, acquire 11.85, submit 4.67; layout_us>0 on 18,281/18,281 frames; GPU (gpu_q=1 on 18,241) total p50/p95 2.58/4.66 ms

**No per-frame winner declared**: both sustain ~61–62 fps with no 60 Hz misses on work. Frust 5.31 ms CPU+GPU-wait (GPU p50/p95 2.58/4.66 ms, rebuild 0.57 ms) vs Flutter 2.74 ms build+raster CPU (GPU unexposed); Flutter's worst is tighter (5.7 vs 11.0 ms). No screenshot exists on iOS to confirm Frust's S5 content here — see the OnePlus 9 S5 caveat and the renderer-transition verdict before reading this row.

### S6 — Text shaping stress

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |
|---|---|---|---|---|---|---|---|
| Frust (profile) † | 17.08 | 17.91 | 18.41 | 24.67 | 15,487 (84.78%) | 18,154 (99.38%) | 18,267 (~60.9 fps avg) |
| Frust CPU work (total − acquire wait) | 5.54 | 6.28 | 6.73 | 15.24 | 0 (0.00%) | 12 (0.07%) | 18,267 (~60.9 fps avg) |
| Flutter (profile) | 4.27 | 5.13 | 5.47 | 13.98 | 0 (0.00%) | 21 (0.11%) | 18,631 (~62.1 fps avg) |

Frust pass p50 (ms): rebuild 0.07, layout 0.15, paint 0.26, encode 0.08, acquire 11.53, submit 4.98; layout_us>0 on 18,267/18,267 frames; GPU (gpu_q=1 on 18,227) total p50/p95 1.64/1.86 ms

**No per-frame winner declared**: both at ~61–62 fps with zero 60 Hz misses on work. Frust 5.54 ms CPU+GPU-wait (GPU p50/p95 1.64/1.86 ms; text shaping in layout/paint 0.4 ms) vs Flutter 4.27 ms build+raster CPU (GPU unexposed); worst 15.2 vs 14.0 ms.

### S7 — Cold start + idle (30 s runs)

| Metric | Frust (profile) | Flutter (profile) |
|---|---|---|
| External cold start (`am start -W` TotalTime, median of kept launches) | not captured | not captured |
| Framework-reported first-frame span | ~110 ms median (`first_frame_presented`, 78–250 across 12 launches; `adapter_ready` median 102 ms) | ~8 ms median (`first_frame_ms`, 6–9 across 12 launches) |
| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | n/a | n/a |
| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | n/a | n/a |

Only the framework-self-reported spans exist on iOS (deviation 2): **Flutter's is far shorter** (8 vs 110 ms median); Frust's span is dominated by GPU adapter/device init (`adapter_ready` median 102 of the 110 ms). No external cold start, idle CPU or memory captured.

### S8 — Plugin-call overhead (shared preferences)

Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).

| Op | Frust (profile) (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | 6 | 125 | n/a |
| write i64 | 15 | 60 | n/a |
| write f64 | 17 | 55 | n/a |
| write String | 6 | 52 | n/a |
| write Vec\<String\> | 6 | 61 | n/a |
| read (unique key, forces channel) bool | 1 | 2698† | ~0 |
| read (unique key, forces channel) i64 | 1 | 2698† | ~0 |
| read (unique key, forces channel) f64 | 1 | 2698† | ~0 |
| read (unique key, forces channel) String | 1 | 2698† | ~0 |
| read (unique key, forces channel) Vec\<String\> | 1 | 2698† | ~0 |

`s8-errors` marker lines across kept logs: frust 0, flutter 0. †Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).

**Frust wins S8 decisively** — writes 6–17 µs vs Flutter's 52–125 µs (≈3–20× per type); Frust's per-key read is ~1 µs vs Flutter's 2,698 µs `reload()` whole-store re-read. Zero errors on both apps.

### Methodology deviations (this device)

1. **Matrix ran in two driver sessions.** The first (`matrix-iphone_se`) was killed externally at 23:48:44Z (an orchestration-tool process-group kill, not a device fault) during frust/s7 run 9 of 12, after S1–S6 (12 blocks) had completed first-attempt `ok`. S7–S8 were re-run in a second session (`matrix-iphone_se-2`, `--scenarios s7,s8`) starting 23:52:17Z, 9 min after the S6 Flutter block ended; its first block reinstalled the frust app from the same artifact and each Flutter `.app` was installed by its block as usual. The first session's 8 partial frust/s7 runs are excluded; S7/S8 are the second session's 12 runs each.
2. **No external cold start, idle CPU or idle PSS on iOS** (no `am start -W`/`dumpsys` equivalents); only framework spans reported.
3. **Environmental controls uncontrolled:** brightness, airplane/Wi-Fi/BT, charger (USB-connected, charging 83→100 %), thermal (no sensor; fixed 60 s cooldown).
4. **Visual gate skipped** (no screenshot CLI); the driver's marker/line-count sanity stood in.
5. **60 Hz panel:** 8.33 ms column N/A; Frust † totals fold in the CADisplayLink acquire wait and pin to the cadence — per-frame comparisons use the CPU-work row (total − acquire); Flutter's total is the `build+raster` work-sum (`totalSpan` negative under load). The CPU-work row is CPU+GPU-wait for Frust and CPU-only for Flutter (no Flutter GPU timing exists), so per-frame rows are reported side by side without a winner.
6. **S3:** Flutter per-op capture 0 frames (structural `addTimingsCallback` gap, as every prior pass). The engine emits a frame's raw line after the `s3-*` end marker, so Frust's per-op rows are shifted by one op (op N's frame lands in op N+1's window) — recorded, not re-run. Flutter kept run-10 captured only 5 `create1k` cycles / 26 frames (others 20 / 98–100); included as captured.
7. Flutter release-mode in-app cross-check not captured; S8 burst-during-animation variant not run (as on every prior pass).

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

## App size (release) — re-measured 2026-09-05 (engine build)

Host-side snapshot via `benchmarks/harness/app_size.sh` over fresh release
builds of `f64be636` (Frust `frust build apk --release` / `frust build ios
--release`, Flutter 3.47.2 `flutter build apk --release [--split-per-abi]` /
`flutter build ios --release`). The Frust Android release was signed with a
throwaway local keystore purely to satisfy the CLI's release-signing gate; signing
does not change the size class.

| Axis | Frust | Flutter |
|---|---|---|
| Android universal release APK (3 ABIs) | **31.46 MB** (32,991,926 B) | 49.36 MB (51,762,140 B) |
| Android arm64-v8a split | **10.94 MB** (11,471,451 B) | 17.45 MB (18,295,458 B) |
| arm64 `.so` payload (from the split) | 10.82 MB (one `libfrustbench.so`) | 16.55 MB (engine + app) |
| dex total | 0.17 MB | 0.81 MB |
| iOS release `.app` (`du -sk`) | **11.61 MB** | 16.64 MB |

Frust stays the smaller artifact on every axis, but by a narrower margin than
the 2026-07-21 vello-era snapshot (universal 21.25 MB → 31.46 MB, arm64 `.so`
7.42 MB → 10.82 MB, iOS 7.96 MB → 11.61 MB). The bench app itself also grew
between the two snapshots (the `d1`/`d2` DB scenarios pull in `frust-database`
with a bundled SQLite), so the delta is not attributable to the renderer alone;
a size attribution of the engine build (`naga` shader translation, `wgpu` 30,
the strip pipeline) against the vello build is the open follow-up.

## Renderer transition (vello → frust-engine) — regression check, 2026-09-05

Every row is `harness/compare.py` over two `summarize.py` JSON dumps: the retired series (git history, `git show f64be636:benchmarks/raw/…`) against this pass's committed raws; Δ positive = slower. The same-build-mode (`--profile`) vello rows are the renderer comparison proper; the 2026-07-21 release rows are cross-methodology (PROTOCOL §2.5) and read on tails only; the engine-to-engine rows show drift since the last engine gate.

### Verdict

**No per-frame renderer regression on Android or iOS in the like-for-like rows; S5 is invalid on the engine build until its empty-content behaviour is fixed; S3 per-op attribution needs a measurement fix; two iOS tail items to inspect.**

- **OnePlus 9, same build mode (vello `--profile` 2026-09-01 → engine `--profile` 2026-09-05):** the
  engine is faster on every scenario that has a vello-profile baseline — S1 p50 14.84→8.20 ms
  (−45 %), S2 17.99→8.83 (−51 %), S5 22.07→7.78 (−65 %), S6 8.19→8.19 (flat, p95 +1 %) — and every
  cell now holds a locked 120 fps where vello ran 47–68 fps. Engine-to-engine drift since the
  Phase-7 gate is −3…−10 % (better) on every metric. Flutter drifted <7 % on every p50/p95 between
  its 3.44.2 and 3.47.2 builds, so the cross-app verdicts are not an artefact of the Flutter move.
- **OnePlus 9, cross-methodology (vello RELEASE 2026-07-21 → engine profile):** S1/S2/S5 tails
  −22…−56 %; S3 per-op p50/p95 −10…−22 %; external cold start 185→135 ms and first frame
  128→105 ms. The two rows that moved the wrong way are §2.5 artefacts, not renderer work:
  S6 p50 5.67→8.19 ms is the 120 Hz pin (the frame period is 8.33 ms; vello's unpinned S6 ran
  ~104 fps; the same-build S6 row above is flat) and S8 writes 65–67→94–95 µs are plugin calls that
  never touch the renderer (profile instrumentation + the OS change; Frust still 3× Flutter).
- **iPhone SE, same build mode (S1/S2/S6):** the 60 Hz cadence is identical (16.6–19.5 ms totals),
  and the engine's per-frame cost is now fully observable — CPU 0.14–2.7 ms + GPU (Metal
  timestamps, `gpu_q=1`) 4.4 / 3.1 / 1.6 ms for S1 / S2 / S6, unchanged from the Phase-7 gate
  (4.35 / 3.07 / 1.64). The "CPU work" row rises +8…+22 % against vello only because the engine's
  `submit_us` waits on the GPU inside it while vello's compute was asynchronous and never
  measured; it is not a like-for-like regression.
- **iPhone SE, inspect (cross-methodology, release → profile):** (1) **S3 table ops** — the
  reconcile-frame tail lengthened: overall series p95 16.60→18.78 ms, p99 16.80→24.89 ms, and
  646 of 18,276 kept frames now land at 20–33 ms (8 before), i.e. ~3.5 % of frames take two vsyncs
  during create/update/clear; per-op p95s moved from 16.3–16.5 to 16.9–17.1 ms. (2) **S7 first
  frame** 87→110 ms median (engine pipeline/shader setup on Metal). Both exceed §2.5's ~3–7 %
  tail bound and deserve a targeted look; neither changes a Frust-vs-Flutter verdict.
- **S5 is NOT a valid comparison on the engine build (visual gate, OnePlus 9).** Frust's S5 screen
  shows one or two image cells and empty background where Flutter shows a continuous column of cells
  (grey placeholders while decoding, then images); the operator's hand launch measured content
  covering 43 / 51 / 0 / 0 / 68 % of the list at 3 / 8 / 13 / 18 / 25 s, and the 2026-09-01 engine
  baseline carries the same signature. `frust_bench`'s S5 draws no placeholder for an undecoded
  cell, so either the engine's image path drops cells or decode never catches up with the scroll —
  either way the engine's cheap S5 frames (7.8 ms at 120 fps) are partly empty frames, and the
  −65 % S5 row above must not be read as a renderer win until the cause is fixed and S5 re-run.
- **S3 per-op attribution is broken on the engine build (measurement, not performance).** The
  engine emits a frame's `frust-perf raw` line *after* the scenario's `bench-scenario-end` marker
  (vello emitted it in between), so `create 1k` collects zero frames and every other op's bucket
  receives the previous op's reconcile frame. The frust per-op rows are shifted by one op on every
  engine-pass device; the overall S3 series is unaffected. Fix in the bench app or `stats.py`
  (attribute by frame start), then the per-op rows become readable again.
- **Pixel 5:** no engine-pass series (the device dropped off USB during its first block); its
  vello-profile baseline (S1/S2/S5/S6) stays in git history for the re-run.

### OnePlus 9 (Adreno 660, 120 Hz pinned)

**same build mode — THE renderer comparison (S1/S2/S5/S6)** — vello classic, profile, 2026-09-01 (tier A/B) → 2026-09-05 engine:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
| S1 | 14.84→8.20 | -44.7% | 16.03→9.45 | -41.1% | 16.85→10.11 | 75.61→47.71 | 1.22%→0.10% | 68.0→120.1 |
| S2 | 17.99→8.83 | -50.9% | 19.85→11.45 | -42.3% | 20.76→13.09 | 51.13→49.98 | 81.11%→0.10% | 67.0→120.0 |
| S5 | 22.07→7.78 | -64.8% | 23.37→8.85 | -62.1% | 24.08→10.41 | 56.39→53.00 | 96.56%→0.10% | 47.5→120.2 |
| S6 | 8.19→8.19 | +0.0% | 8.78→8.90 | +1.4% | 9.43→10.50 | 52.12→49.64 | 0.15%→0.12% | 119.9→120.2 |

**Moved past ±10% (inspect):**
- S1 p50 -44.7% (14.84→8.20 ms)
- S1 p95 -41.1% (16.03→9.45 ms)
- S2 p50 -50.9% (17.99→8.83 ms)
- S2 p95 -42.3% (19.85→11.45 ms)
- S5 p50 -64.8% (22.07→7.78 ms)
- S5 p95 -62.1% (23.37→8.85 ms)

**engine-to-engine drift since Phase 7** — engine, profile, 2026-09-01 (p7-06) → 2026-09-05 engine:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
| S1 | 8.43→8.20 | -2.7% | 9.84→9.45 | -4.0% | 10.50→10.11 | 50.10→47.71 | 0.11%→0.10% | 120.0→120.1 |
| S2 | 9.29→8.83 | -5.0% | 11.24→11.45 | +1.9% | 12.60→13.09 | 48.09→49.98 | 0.12%→0.10% | 120.1→120.0 |
| S5 | 8.59→7.78 | -9.5% | 9.38→8.85 | -5.6% | 10.53→10.41 | 52.77→53.00 | 0.07%→0.10% | 120.1→120.2 |
| S6 | 9.08→8.19 | -9.8% | 9.52→8.90 | -6.5% | 10.60→10.50 | 51.25→49.64 | 0.21%→0.12% | 120.1→120.2 |

**No metric moved past ±10%.**

**cross-methodology (§2.5: medians not comparable, tails within ~3–7 %)** — vello, RELEASE, 2026-07-21 (full matrix) → 2026-09-05 engine:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
| S1 | 9.70→8.20 | -15.5% | 12.19→9.45 | -22.5% | 15.49→10.11 | 56.49→47.71 | 0.77%→0.10% | 64.5→120.1 |
| S2 | 10.87→8.83 | -18.8% | 14.97→11.45 | -23.5% | 17.96→13.09 | 56.12→49.98 | 1.64%→0.10% | 60.2→120.0 |
| S4 | 4.64→4.54 | -2.2% | 5.50→6.22 | +13.1% | 6.11→8.26 | 57.61→64.53 | 0.06%→0.07% | 119.7→120.2 |
| S5 | 15.71→7.78 | -50.5% | 20.08→8.85 | -55.9% | 20.94→10.41 | 56.23→53.00 | 41.80%→0.10% | 47.9→120.2 |
| S6 | 5.67→8.19 | +44.3% | 10.75→8.90 | -17.2% | 12.14→10.50 | 58.14→49.64 | 0.07%→0.12% | 103.7→120.2 |
| S3 (overall series) | 8.99→5.57 | -38.1% | 12.20→12.16 | -0.3% | 20.43→19.25 | 54.34→47.08 | 2.76%→2.14% | 0.0→0.0 |

**Moved past ±10% (inspect):**
- S1 p50 -15.5% (9.70→8.20 ms)
- S1 p95 -22.5% (12.19→9.45 ms)
- S2 p50 -18.8% (10.87→8.83 ms)
- S2 p95 -23.5% (14.97→11.45 ms)
- S4 p95 +13.1% (5.50→6.22 ms)
- S5 p50 -50.5% (15.71→7.78 ms)
- S5 p95 -55.9% (20.08→8.85 ms)
- S6 p50 +44.3% (5.67→8.19 ms)
- S6 p95 -17.2% (10.75→8.90 ms)
- S3 (overall series) p50 -38.1% (8.99→5.57 ms)
- S3 s3-clear p50 -15.6%
- S3 s3-clear p95 -21.6%
- S3 s3-create10k p95 -18.0%
- S3 s3-swap p50 -11.5%
- S3 s3-swap p95 -16.5%
- S3 s3-update p50 -10.1%
- S3 s3-update p95 -19.7%
- S7 first-frame span -18.0% (128→105 ms)
- S8 write:f64 +40.3% (67→94 µs)
- S8 write:string +46.2% (65→95 µs)
- S8 write:string_list +41.8% (67→95 µs)

**Flutter drift (3.44.2 → 3.47.2, OxygenOS → LineageOS)** — Flutter 3.44.2, 2026-07-21 → 2026-09-05 engine:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
| S1 | 9.23→9.44 | +2.3% | 16.90→17.15 | +1.4% | 19.08→19.47 | 38.61→40.11 | 5.10%→5.19% | 101.5→97.3 |
| S2 | 14.57→15.20 | +4.4% | 23.97→24.20 | +0.9% | 27.09→27.86 | 44.59→82.34 | 31.53%→37.97% | 20.7→19.5 |
| S4 | 4.23→4.24 | +0.2% | 5.01→4.94 | -1.4% | 5.79→5.89 | 14.94→74.91 | 0.00%→0.00% | 119.5→119.5 |
| S5 | 6.14→6.54 | +6.6% | 13.87→14.21 | +2.5% | 15.96→16.35 | 23.27→26.03 | 0.44%→0.78% | 112.2→108.0 |
| S6 | 8.35→8.37 | +0.3% | 9.75→9.81 | +0.7% | 10.19→10.29 | 11.93→91.13 | 0.00%→0.01% | 119.2→119.2 |
| S3 (overall series) | 19.32→19.16 | -0.8% | 34.90→33.65 | -3.6% | 35.94→36.07 | 81.30→83.64 | 65.29%→64.39% | 0.0→0.0 |

**No metric moved past ±10%.**

### iPhone SE (A13, 60 Hz)

**same build mode — renderer comparison on the WORK row (S1/S2/S6)** — vello classic, profile, 2026-09-01 (tier A/B) → 2026-09-05 engine:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
| S1 | 4.76→5.79 | +21.6% | 5.32→6.48 | +21.7% | 5.67→6.92 | 9.82→13.04 | 0.00%→0.00% | 0.0→0.0 |
| S2 | 5.73→6.34 | +10.6% | 6.72→7.33 | +9.1% | 7.11→7.73 | 10.21→11.64 | 0.00%→0.00% | 0.0→0.0 |
| S6 | 5.14→5.54 | +7.7% | 5.66→6.28 | +11.0% | 5.95→6.73 | 13.68→15.24 | 0.00%→0.00% | 0.0→0.0 |

**Moved past ±10% (inspect):**
- S1 p50 +21.6% (4.76→5.79 ms)
- S1 p95 +21.7% (5.32→6.48 ms)
- S2 p50 +10.6% (5.73→6.34 ms)
- S6 p95 +11.0% (5.66→6.28 ms)

**engine-to-engine drift (work row)** — engine, profile, 2026-09-01 (p7-06) → 2026-09-05 engine:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
| S1 | 5.72→5.79 | +1.2% | 6.39→6.48 | +1.4% | 6.84→6.92 | 16.04→13.04 | 0.00%→0.00% | 0.0→0.0 |
| S2 | 6.43→6.34 | -1.5% | 7.40→7.33 | -1.0% | 7.94→7.73 | 12.58→11.64 | 0.00%→0.00% | 0.0→0.0 |
| S6 | 5.43→5.54 | +2.1% | 6.29→6.28 | -0.0% | 6.75→6.73 | 14.56→15.24 | 0.00%→0.00% | 0.0→0.0 |

**No metric moved past ±10%.**

**cross-methodology, work row** — vello, RELEASE, 2026-07-21 (full matrix) → 2026-09-05 engine:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
| S1 | 4.41→5.79 | +31.3% | 5.16→6.48 | +25.6% | 5.87→6.92 | 39.27→13.04 | 0.01%→0.00% | 0.0→0.0 |
| S2 | 4.80→6.34 | +32.0% | 6.00→7.33 | +22.1% | 6.36→7.73 | 8.75→11.64 | 0.00%→0.00% | 0.0→0.0 |
| S4 | 3.58→3.42 | -4.7% | 5.11→4.03 | -21.0% | 5.36→4.28 | 6.79→15.44 | 0.00%→0.00% | 0.0→0.0 |
| S5 | 4.67→5.31 | +13.5% | 5.25→6.58 | +25.4% | 5.57→7.36 | 9.28→10.99 | 0.00%→0.00% | 0.0→0.0 |
| S6 | 4.67→5.54 | +18.7% | 5.41→6.28 | +16.2% | 5.67→6.73 | 9.79→15.24 | 0.00%→0.00% | 0.0→0.0 |
| S3 (overall series) | 16.22→16.79 | +3.5% | 16.60→18.78 | +13.2% | 16.80→24.89 | 38.91→31.71 | 2.70%→62.32% | 0.0→0.0 |

**Moved past ±10% (inspect):**
- S1 p50 +31.3% (4.41→5.79 ms)
- S1 p95 +25.6% (5.16→6.48 ms)
- S2 p50 +32.0% (4.80→6.34 ms)
- S2 p95 +22.1% (6.00→7.33 ms)
- S4 p95 -21.0% (5.11→4.03 ms)
- S5 p50 +13.5% (4.67→5.31 ms)
- S5 p95 +25.4% (5.25→6.58 ms)
- S6 p50 +18.7% (4.67→5.54 ms)
- S6 p95 +16.2% (5.41→6.28 ms)
- S3 (overall series) p95 +13.2% (16.60→18.78 ms)
- S7 first-frame span +26.4% (87→110 ms)
- S8 write:bool +100.0% (3→6 µs)
- S8 write:f64 +325.0% (4→17 µs)
- S8 write:i64 +400.0% (3→15 µs)
- S8 write:string +100.0% (3→6 µs)

**Flutter drift (build+raster work-sum)** — Flutter 3.44.2, 2026-07-21 → 2026-09-05 engine:

| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |
| S1 | 4.92→4.91 | -0.3% | 5.50→5.56 | +1.1% | 5.87→5.71 | 25.26→20.82 | 0.04%→0.04% | 61.8→61.9 |
| S2 | 4.87→4.83 | -0.9% | 5.43→5.52 | +1.5% | 5.91→5.84 | 11.13→11.07 | 0.00%→0.00% | 21.4→21.4 |
| S4 | 1.34→1.34 | -0.2% | 1.59→1.51 | -4.9% | 1.71→1.63 | 7.63→6.82 | 0.00%→0.00% | 61.8→62.1 |
| S5 | 2.71→2.74 | +1.0% | 3.61→3.67 | +1.9% | 3.90→3.95 | 5.52→5.74 | 0.00%→0.00% | 61.6→62.0 |
| S6 | 4.24→4.27 | +0.6% | 4.96→5.13 | +3.4% | 5.26→5.47 | 16.80→13.98 | 0.01%→0.00% | 61.9→62.1 |
| S3 (overall series) | 4.97→5.01 | +0.9% | 12.33→12.84 | +4.1% | 14.67→13.72 | 18.62→16.80 | 0.10%→0.22% | 0.0→0.0 |

**Moved past ±10% (inspect):**
- S7 first-frame span +14.3% (7→8 ms)

### Pixel 5 (Adreno 620, unpinned 60/90 Hz)

_No engine-pass summary for this device (run not completed)._

## DB scenarios (`d1`/`d2`) — no runs recorded yet

`PROTOCOL.md` §9 specifies the `d*` op-latency class (`d1` batched/autocommit
writes, `d2` point SELECT + range scan) against the fixed row shape and seed
dataset of §9.2, on §7's per-op raw-line contract. **No device has run it.**
When a pass is captured this section gains one subsection per device with a
`d1` and a `d2` table (per-op p50/p95/p99 µs + ops/s) across three columns —
Frust `frust-database` (in-process `rusqlite`), Flutter `package:sqlite3`
(engine-parity FFI) and Flutter `sqflite` (platform channel) — each side's
linked SQLite version recorded per run, plus a methodology-deviations list.
