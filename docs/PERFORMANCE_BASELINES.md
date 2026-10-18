# Performance Baselines

What is measured: release `.so` size and clean-rebuild build time, plus the Dev Loop and benchmark
suites. How: `scripts/size-report.sh`, `benchmarks/harness/size_attribute.py`, and a timed
`cargo clean -p <crate> --release` rebuild (warm cache, median of 3, bash `time`). Host class: an
i5-12600 Linux desktop; Android is arm64-v8a via `cargo ndk` with `ANDROID_HOME`/`ANDROID_NDK_HOME`
set. Reference commit for the pre-erasure comparison: main 79686b68. The numbers are data points for
the release notes, not a gate. Build, run and test commands live in [DEVELOPMENT.md](DEVELOPMENT.md).

## Release profile

**Release-profile hardening.** `[profile.release]` (`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "abort"`, the global `opt-level` left at its default 3) is hand-synced across **ten** manifests: root, `crates/frust-drive/templates/app/Cargo.toml.tmpl`, `examples/{huddle,glyph-catalog,material3-demo,playground,shadertoy,native-widgets-demo,web-gallery}` and `benchmarks/frust_bench`. The **nine** that build an Android artifact (the ten minus the wasm-only `web-gallery`) additionally carry an identical `[profile.release.package.<crate>] opt-level = "z"` **cold set**: `naga`, `codespan-reporting`, `ash`, `gpu-allocator`, `serde_json`, `roxmltree`, `fontique`, `mio`, `image`, `png`. `frust-engine`, `frust-gpu`, `frust-render`, `frust-text`, `harfrust`, `skrifa`, `read-fonts`, `zune-jpeg` and `parley` stay at 3. `jni` stays at 3 too: its IME, clipboard-drain and system-UI polls (once per frame, from `FrustSurfaceView.kt`'s `doFrame`) run in separate JNI entries after `nativeOnFrame` returns, outside every span the benchmark's `total_us` sums, so the bar cannot measure it. `wgpu-core` and `wgpu-types` stay at 3 too: `wgpu-core` sits on the per-frame queue-write/submit path and its `z` build cost about 0.2 ms of S5's `submit` p50 (see [RESULTS.md](../benchmarks/RESULTS.md)'s S5 attribution note), and `z` made `wgpu-types` larger, not smaller. `tokio`, `accesskit`, `accesskit_consumer` and `accesskit_android` stay at 3 too, settled by microbench instead of the frame-span bar (see [RESULTS.md](../benchmarks/RESULTS.md)'s tokio/accesskit microbench note): accesskit's publish failed the cold set's CPU rule at `z` on a little core (at a 2,000-node tree), and `z` saved `tokio` no size. `wgpu-hal` is excluded by measurement — on the submit path, it cost consistent per-frame CPU for 0.55% of the `.so`. Measured on the benchmark app's lean arm64 release against the original cold set — `jni`, `tokio`, the three `accesskit` crates, `wgpu-core` and `wgpu-types` still in it (at 3, `jni` alone added 0.44% to the `.so`): stripped `.so` −9.2%, `.text` −17.9%, profile APK −7.4%. `jni` leaving the set later added back +30,184 B lean; `tokio` and the three `accesskit` crates leaving added +21,944 B lean; `wgpu-core` and `wgpu-types` leaving added +237,104 B lean (each its own per-lever row — RESULTS.md never sums them); `[profile.profile]` inherits the overrides, so a benchmark build measures the shipping configuration. The global `opt-level` stays 3 because a whole-binary `"s"`/`"z"` never cleared the 5% bar against a render-stack CPU-perf carve-out (*Measuring* below); the cold set is gated on that same bar and clears it on a OnePlus 9 for the frame's own spans (see [RESULTS.md](../benchmarks/RESULTS.md)'s cold-set note). `cargo test -p frust-cli --test profile_sync` keeps every mirror identical — three lists (dev overrides ×5, `[profile.release]` identity ×10, the Android cold set ×9) — and asserts the template's `.cargo/config.toml` `[target.*]` tables match the root's structurally (its extra `[build]` table is covered in *Build Output Layout* in [DEVELOPMENT.md](DEVELOPMENT.md)).

## Android link flags

The repo-root `.cargo/config.toml` adds two linker flags on all four Android targets: `-C link-arg=-Wl,--pack-dyn-relocs=android` (Bionic's packed relocation format, API 23+ — `.rela.dyn` 308 KB → 40 KB on the benchmark `.so`; the newer `android+relr` form would need API 28, above every Frust target's `minSdk 26`) and `-C link-arg=-Wl,--icf=all` (identical-code folding — `all` over the conservative `safe` for a further ~203 KB, accepting a risk class, function-pointer/vtable identity comparison, that this codebase does not rely on). Cargo merges config files from every ancestor directory of the build `cwd`, so that one file governs every nested workspace built inside the checkout; `crates/frust-drive/templates/app/.cargo/config.toml` carries the same `[target.*]` rustflags tables (a verbatim `template_manifest.json` entry, kept structurally identical by `profile_sync`) so a scaffold keeps the flags once it leaves the checkout — plus its own `[build]` table the root deliberately lacks (*Build Output Layout* in [DEVELOPMENT.md](DEVELOPMENT.md)). A caller's `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS` replaces the table entirely. **Unwind tables stay:** `-C force-unwind-tables=no` measures ~9% off the lean arm64 `.so` (`.eh_frame` −72%) but destroys native tombstone/`ndk-stack` frame resolution past the panic-abort entry, and `-C force-frame-pointers=yes` does not restore it.

## Measuring

`scripts/size-report.sh [--app <dir>]` (default `examples/huddle`) builds the arm64-v8a release `.so` via `cargo ndk` and reports unstripped/stripped size, `cargo bloat` breakdowns, and the APK/AAB per-ABI `.so` + dex breakdown when Gradle output exists (missing tools degrade to a note); `benchmarks/harness/size_attribute.py` attributes an unstripped Android release `.so` to crates and ELF sections via the NDK's `llvm-readelf`/`llvm-nm`. Build time: `cargo clean -p <crate> --release`, then a timed `cargo build --release -p <crate>` rebuild (warm dependency cache, median of 3, bash `time`); the baselines below are in the table.

## Recorded baselines

| Date | Ref | Host | App | Stripped .so | Unstripped .so | Clean release rebuild (median of 3) |
|------|-----|------|-----|--------------|----------------|-------------------------------------|
| 2026-10-06 | 0.6 erasure-at-the-API-boundary migration (main f79b739c) | i5-12600 Linux | huddle | 9,069,848 B | 9,070,176 B | frust-gallery 13.6 s (+0.12 % size / +3.4 % build time vs main 79686b68; bars ≤ 2 % / ≤ 10 %) |
| 2026-10-06 | main 79686b68 → f79b739c | i5-12600 Linux | material3-demo | 14,593,400 B → 14,642,912 B (+49,512 B = +0.34 %; bar ≤ 2 %) | 14,593,728 B → 14,643,240 B | – |
| 2026-10-06 | main 79686b68 → f79b739c | i5-12600 Linux | frust-gallery + material3demo (lib + bin, `cargo build --timings`) | – | – | frust-gallery 13.71 s → 13.94 s (+1.6 %; runs 13.793 / 13.713 / 13.479 s → 14.488 / 13.936 / 13.740 s; unit 13.51 s → 13.73 s); material3demo 127.86 s → 124.18 s (−2.9 %; runs 125.669 / 127.858 / 128.037 s → 124.101 / 124.269 / 124.183 s; unit 127.65 s (lib ≈ 45.5 s + bin ≈ 82 s) → 123.97 s); bar ≤ 10 % |
| 2026-10-07 | 0.7 erasure-free app code: main 721ab1e0 → feature/erasure-free-apps b2a41334 | i5-12600 Linux | material3-demo | 14,643,024 B → 14,658,576 B (+15,552 B = +0.11 %; bar ≤ 2 %) | 14,643,352 B → 14,658,904 B | – |
| 2026-10-07 | main 721ab1e0 → b2a41334 | i5-12600 Linux | frust-gallery + material3demo (lib + bin, `cargo build --timings`) | – | – | frust-gallery 14.42 s → 13.74 s (−4.7 %; runs 17.687 / 14.333 / 14.421 s → 13.749 / 13.740 / 13.725 s; unit 14.22 s → 13.54 s); material3demo 129.02 s → 125.77 s (−2.5 %; runs 128.554 / 131.012 / 129.016 s → 128.826 / 125.771 / 124.955 s; unit 128.81 s → 125.55 s); bar ≤ 10 % |

The 2026-10-07 rows were taken with the host otherwise idle except for the first frust-gallery run on each side, which overlapped the tail of a preceding gate run (1-min load 7.97 base / 5.93 head at that run, 1.1–2.1 for the material3demo runs); medians are unaffected by the one slow outlier. Sizes come from separate detached worktrees and `CARGO_TARGET_DIR`s (`target-size-base`/`target-size-head`). The 2 % size and 10 % build-time figures are report thresholds — data points that trigger a note to the approver, not gates.

The unstripped figure is within 328 B of the stripped one because `strip = "symbols"` already strips
the release profile. Each side was built in its own detached worktree and target dir (rustc 1.98.1,
cargo-ndk 4.1.2, NDK 28.2.13676358; wall time by bash `time`, unit time from the `--timings` report).
Method deviations: the base-side material3demo runs 2 and 3 overlapped a ~40 s unrelated test build
and the head-side frust-gallery run 1 a short formatting check; medians are reported, so neither
changes the figure.

### frust-file-dialogs (rfd) cost

2026-10-18, i5-12600 Linux, rfd 0.17.2 default features (`xdg-portal`, `wayland`), rustc warm: a scratch `bin` linking
`frust-file-dialogs` (`.open_file()` kept live behind an impossible runtime branch) against an empty `fn main() {}`, both
with the workspace `[profile.release]` (`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "abort"`, opt 3).
Stripped size 301,712 B → 564,344 B (**+262,632 B**, +87 %); clean `cargo build --release` 4.80 s → 63.02 s (**+58.2 s**;
one run each, bash timestamps, per-side target dir). The delta is a desktop-only plugin cost; Android, iOS and web never compile `rfd`.

## Engine bench scenes

`crates/frust-engine/benches/` carries three phone-shaped scenes recorded straight through
`frust_scene::SceneBuilder` (never the bench app) and compiled at 1080x2340 by a long-lived
`SceneCompiler`: `s1_like` (60 radial-gradient circles at 0.7 to 0.3 alpha with a 0.6-alpha 2 px stroke,
moved every iteration, plus 120 short glyph runs), `s6_like_cached` (one 2,009-glyph paragraph at 18 px,
laid out once, re-emitted unchanged) and `s6_like_rebreak` (the same paragraph, wrap width alternating
between 1000 and 760 px per iteration, re-break untimed), `s5_like` (nine 256x256 RGBA images magnified
into 1064x256 rows under a rect clip, same images every iteration). Text is shaped once with `frust-text`
through a `TextContext` whose fallback chain is the host's font collection: the bundled `testing/fonts` Latin subset
covers only `H e l o`, space, comma and `é`, so the `s6_like` paragraph shapes most of its glyphs against a host
fallback face and those two rows are host-dependent, not a best-case atlas (`s1_like`'s short runs stay on the subset). Commands: `cargo bench -p frust-engine
-- --quick` times `scenes_compile/*` (compile only) and `scenes_encode/*` (compile, upload and submit against
an offscreen target on the host GPU; skipped with a note when no adapter exists or `FRUST_BENCH_NO_GPU` is
set); add `--features perf-trace` for the span rows below (medians of warm 60-frame `frust-perf enc`
windows over 180 untimed frames, plus the compile spans).

| Ref | Host | Scene | Compile p50 (ms) | walk / glyphs (us) | upload (us) | replay (us) | draws / strips / alphas / glyph_draws |
|-----|------|-------|------------------|--------------------|-------------|-------------|----------------------------------------|
| feature/frust-fast-p0 eff23e65 | i5-12600 Linux, NVIDIA T400 (Vulkan) | `s1_like` | 1.17 | 1152 / 250 | 241 | 0.1 | 637 / 9,560 / 296,448 / 516 |
| same | same | `s6_like_cached` | 1.18 | 1090 / 1057 | 293 | 0.1 | 1,710 / 12,295 / 273,872 / 1,709 |
| same | same | `s6_like_rebreak` | 1.18 | 1096 / 1064 | 294 | 0.1 | 1,710 / 12,295 / 273,872 / 1,709 |
| same | same | `s5_like` | 0.158 | 157 / 0 | 64 | 0.1 | 10 / 2,326 / 71,104 / 0 |

Compile p50 is the encode's `compile_us`; the `scenes_encode` criterion mean (CPU wall of encode, submit and
`end_frame`) was 1.37 / 1.66 / 1.66 / 0.32 ms for the four rows, and `scenes_compile` alone 1.16 / 1.04 / 1.04 /
0.051 ms. Measured on an otherwise idle host (1-minute load about 3 on 12 threads) with
`cargo bench -p frust-engine --features perf-trace -- scenes --warm-up-time 2 --measurement-time 5`; a run taken
while other builds were active moved the upload and `s5_like` figures by up to 2x, so read before/after pairs from
back-to-back runs on a quiet host. Replay is near zero because every glyph is atlas-resident after the warm-up
frames; the six atlas counts appended to the enc line (`replays`, `dirty_pages`, `atlas_refused`, `glyph_misses`,
`glyph_evicted`, `run_keys`) read 0 / 0 / 0 / 0 / 0 / 2 for the text scenes (re-verified at 9e510235, where
`glyph_misses` first counted the production path's misses).

## Engine encode and GPU on two phones

Before-numbers for the frust-fast plan, captured 2026-10-13 from `feature/frust-fast-p0` (eff23e65) with the
protocol profile APK (`FRUST_TRACE_RAW=1`), `benchmarks/harness/run.sh <sN> --runs 12 --duration 30` (first two
runs discarded; S3 at 4x20). Columns: harness total p50 / p95, `submit_us` p50, `gpu_main_us` p50 / p95 (ms); enc
medians over the kept 60-frame windows for walk / glyphs / upload / replay / instances (us); draws / strips /
alphas. Fairness gate warnings on both phones: airplane mode off, charging over USB (uncontrolled, identical
across rows). Raw trees stay out of `benchmarks/raw`.

Pixel 4a class (Adreno 618, 1080x2340 @ 60 Hz):

| Scenario | total p50 / p95 | submit p50 | gpu_main p50 / p95 | walk / glyphs / upload / replay / instances | draws / strips / alphas |
|---|---|---|---|---|---|
| S1 | 33.52 / 35.04 | 16.16 | 32.70 / 33.16 | 11536 / 6570 / 649 / 1016 / 362 | 536 / 14,788 / 494,448 |
| S2 | 19.18 / 21.30 | 7.08 | 17.00 / 17.42 | 3480 / 2783 / 456 / 0.7 / 293 | 797 / 11,736 / 283,920 |
| S3 | 13.56 / 16.26 | 5.97 | 10.35 / 11.03 | 1874 / 1640 / 399 / 0.8 / 130 | 356 / 4,981 / 115,744 |
| S4 | 12.89 / 13.85 | 5.18 | 7.21 / 7.78 | 695 / 315 / 311 / 0.8 / 43 | 46 / 1,056 / 26,832 |
| S5 | 33.31 / 37.01 | 6.05 | 26.27 / 26.68 | 1889 / 183 / 470 / 0.9 / 102 | 31 / 3,050 / 84,512 |
| S6 | 13.95 / 17.55 | 7.28 | 13.02 / 14.46 | 3382 / 3072 / 380 / 0.8 / 268 | 512 / 8,925 / 216,120 |

Pixel 4 class (Adreno 640, 1080x2280 @ 90 Hz; S1 ran warm — the phone sat at 38.8-39.6 C past the gate's
120 s wait and its per-run frame count fell from 2650 to 2050, so the cool-phone S1 figure is the spike's 10.96):

| Scenario | total p50 / p95 | submit p50 | gpu_main p50 / p95 | walk / glyphs / upload / replay / instances | draws / strips / alphas |
|---|---|---|---|---|---|
| S1 | 14.18 / 16.52 | 13.77 | 11.15 / 11.38 | 10978 / 8804 / 316 / 1036 / 166 | 525 / 14,076 / 474,320 |
| S2 | 8.74 / 9.91 | 6.15 | 8.00 / 9.24 | 2669 / 2126 / 399 / 0.3 / 224 | 796 / 11,336 / 281,336 |
| S3 | 6.04 / 11.54 | 4.99 | 5.02 / 5.91 | 1691 / 1490 / 296 / 0.4 / 122 | 355 / 4,993 / 113,856 |
| S4 | 4.41 / 4.99 | 3.88 | 2.02 / 2.21 | 624 / 297 / 244 / 0.3 / 39 | 45 / 1,025 / 27,696 |
| S5 | 6.32 / 12.60 | 4.92 | 8.54 / 8.82 | 1840 / 129 / 586 / 0.3 / 72 | 30 / 3,001 / 92,448 |
| S6 | 7.21 / 7.84 | 5.90 | 7.32 / 9.46 | 2373 / 2192 / 347 / 0.2 / 187 | 508 / 8,918 / 224,352 |

Cold start (S7, `first_frame_presented` medians): Pixel 4a 211 ms of which `first_submit_pipeline_wait` 118,
encode 6, queue 3; Pixel 4 172 ms of which pipeline wait 92, encode 5, queue 2. On both phones the pipeline
cache is persisted 328 B about 70 ms after surface install with zero warm-up pipelines finished, and the second
launch loads that same 328 B (the after-warm-up blob is in the close-out section below). S1's atlas on both
phones (re-captured at 9e510235 with `glyph_misses` on the production path, 6x30 s, first two runs discarded):
every drawn glyph is atlas-resident (`glyph_draws == atlas_glyphs`: 466 on the Pixel 4a, 458 on the Pixel 4),
yet per 60-frame window the Pixel 4a replays on 37 frames with 66 misses and 69 evictions and the Pixel 4 on 40
frames with 81 misses and 93 evictions, and nothing is refused. Replay is gated on misses by construction, so
the before-number to beat is the miss count; in steady state the misses roughly equal the evictions (1.8-2.1 per
replaying frame). The misses were sibling subpixel buckets of on-screen glyphs aging out and being
re-rasterised; keeping the siblings took them to zero (close-out section below). Pixel 4 S1 re-captured cool
(32.7 C start): p50 11.66 / p95 15.44 ms, drifting to 35 C by run 6. Thread sample during S1: main,
`RenderThread` and both `frust-reactive` threads at nice -10, `frust-render` at nice -4.

## Engine close-out per device

The frust-fast work against the before-rows above, one row per phone and scene. Columns: harness
total p50 / p95 and `gpu_main_us` p50 (ms); "after" is the latest split-path capture of that scene
that carries `gpu_main`, at the ref named (`FRUST_TRACE_RAW=1`, 12x30 s with the first two runs
discarded, S3 4x20): rows at c61ee81a and 2acaf544 predate the later levers, which left those scenes
unmeasured, and the later S1 12x30 pairs at 3708c199 carry no `gpu_main` and sit in the first-frame
paragraph. The OnePlus 9 and iPhone SE have no eff23e65 row: their reference is
the 2026-09-06 pass in [RESULTS.md](../benchmarks/RESULTS.md), and the OnePlus 9 rows at 78fb0a4d
carry their v0.6.2 before in brackets. Full tables, kill-switch arms, atlas counters, kgsl clocks
and thermal notes stay in the gate records.

| Phone / scene | before (eff23e65) | after | at |
|---|---|---|---|
| Pixel 4a S1 | 33.52 / 35.04, 32.70 | 11.97 / 16.18, 13.89 | 78fb0a4d |
| Pixel 4a S2 | 19.18 / 21.30, 17.00 | 9.95 / 12.47, 10.61 | c61ee81a |
| Pixel 4a S3 | 13.56 / 16.26, 10.35 | 11.68 / 16.20, 3.10 | c61ee81a |
| Pixel 4a S4 | 12.89 / 13.85, 7.21 | 11.68 / 12.42, 2.35 | 78fb0a4d |
| Pixel 4a S5 | 33.31 / 37.01, 26.27 | 13.07 / 14.61, 13.25 | 78fb0a4d |
| Pixel 4a S6 | 13.95 / 17.55, 13.02 | 12.63 / 17.89, 4.90 | c61ee81a |
| Pixel 4 S1 | 14.18 / 16.52, 11.15 | 5.87 / 11.67, 7.34 | c61ee81a |
| Pixel 4 S2 | 8.74 / 9.91, 8.00 | 5.01 / 6.80, 2.69 | c61ee81a |
| Pixel 4 S3 | 6.04 / 11.54, 5.02 | 4.40 / 11.90, 1.09 | c61ee81a |
| Pixel 4 S4 | 4.41 / 4.99, 2.02 | 3.78 / 4.33, 0.90 | 2acaf544 |
| Pixel 4 S5 | 6.32 / 12.60, 8.54 | 6.41 / 7.17, 5.52 | 2acaf544 |
| Pixel 4 S6 | 7.21 / 7.84, 7.32 | 5.01 / 5.70, 1.48 | c61ee81a |
| OnePlus 9 S1 / S4 / S5 | – | 8.20 / 9.38, 3.47 [8.22 / 9.40]; 4.36 / 6.19, 0.46 [4.37 / 6.20]; 6.25 / 7.01, 2.93 [6.27 / 6.99] | 78fb0a4d |
| OnePlus 9 S2 / S3 / S6 | – | 6.41 / 9.52, 1.34; 4.28 / 12.19, 0.53; 4.96 / 5.80, 0.60 | c61ee81a |
| iPhone SE S1 / S2 / S3 / S6 (CPU work = total − acquire) | – | 5.63 / 6.53, 1.94; 6.00 / 6.95, 1.02; 3.81 / 7.94, 0.31; 5.26 / 6.64, 0.46 | c61ee81a |

Encode spans. Split path at c61ee81a: Pixel 4a S1 walk 11536 to 2111 us and glyphs 6570 to 373, S6
enc total 1870; Pixel 4 S1 enc total 3798; OnePlus 9 S1 / S6 4355 / 1548; iPhone SE S1 / S6 4196 /
2844. Uploads and glyph replay, inline-path protocol (`FRUST_NO_RENDER_THREAD=1`), v0.6.2 to
b6e5a665: Pixel 4a S1 `upload_us` 571 to 216, `replay_us` 1283 to 0.3, enc total 4523 to 2797 (S6
upload 381 to 62, enc 1714 to 1408); OnePlus 9 S1 upload 358 to 121, replay 1184 to 0.3, enc 4103 to
3350 (S6 upload 260 to 47, enc 1489 to 1252); replays / misses / evictions per window 51 / 124 / 132
(Pixel 4a) and 49 / 109 / 127 (OnePlus 9) to zero. `FRUST_ENGINE_NO_STAGING_RING=1` reproduces the
before uploads, so the ring owns the upload gain and the sibling-bucket keep owns the replay one.
Host (i5-12600, T400), per-lever back-to-back pairs taken on each lever's own branch against its
parent in the same session, not against the eff23e65 host table above: `s6_like` glyphs 1167 to
500 us with glyph quads (8683cf25) and 514 to 140 with the resident glyph-run cache (b8b75d4d);
`s1_like` glyphs 96 to 35 (b8b75d4d); `s1_like` walk 875-922 to 387 with the parallel walk
(c61ee81a).

Scheduling and JNI (78fb0a4d, split path): the thread policy is neutral on every S-row (the UI
thread already runs at nice -10 and the render thread now keeps it; `FRUST_NO_THREAD_TUNING=1`
restores `frust-render` at -4 and reproduces the before rows; blocking-pool threads take the
background class, reference +10, on their first job). Per-frame JNI polls on an idle playground
page fell from 166 to 36 us (Pixel 4a) and 140 to 30 us (OnePlus 9). `FRUST_ADPF=1` on the OnePlus
9 (API 35): split path S4 4.22 / 5.91, S1 8.14 / 9.36, within noise; the inline-path A/B at
0d5c7cb3 (12x30, hints on against off) read S4 p95 +4 % and p99 +12 % worse with hints, which kept
the session opt-in (`android-adpf-opt-in-measured` in LIMITATIONS.md); the Pixel 4a reports no hint
manager, so it cannot open one.

First frame (S7, `first_frame_presented` spans, split path, a7a5b074 to 3708c199; `run.sh s7 --runs
12 --duration 20` with the blob present; cold = `pm clear`, then launch 1 and launch 2, 3 trials):

| Phone | enc -> presented warm median [min-max] | pipeline wait warm | first_frame_presented warm | cold launch 1 enc -> presented / wait | launch 2 | blob (pipelines) | `am start -W` TotalTime |
|---|---|---|---|---|---|---|---|
| Pixel 4a | 194 [191-200] -> 24 [14-28] | 183.5 -> 12.5 | 396 -> 219 | 193 / 182 -> 177 / 166 | 14 / 4 | 328 B (0) -> 117143 B (18) | 134 (133/134/144; no prior row) |
| OnePlus 9 | 74 [73-96] -> 11 [10-13] | 66 -> 3 | 180.5 -> 118 | 73 / 66 -> 77 / 69 | 11 / 3 | 314 B (0) -> 116758 B (18) | 131 (138/131/130; 2026-09-06: 128) |

`await` and `finish` make the first frame pay the whole list one pipeline at a time (Pixel 4a 86 / 85
ms warm, ~425 cold; OnePlus 9 43 / 42.5 warm, ~187 cold), so `steal` is the default; the depth-alpha
LayerComposite variant (+32 / +5 ms Pixel 4a cold / seeded, +10 / +2 OnePlus 9) is warmed last. The
blob is written after the warm-up or on the first surface destroy and never rewritten unchanged. S1
steady state at 3708c199 is unchanged (Pixel 4a 11.24 / 13.56 vs 11.29 / 14.59, OnePlus 9 8.02 /
8.87 vs 7.94 / 8.77).

Targets. Met: Pixel 4a S1 `gpu_main` <= 16 ms (12.75 at 2acaf544, the paint-class programs alone,
which also took Pixel 4 S1 `gpu_main` from 11.15 to 8.12 — the pair the 0.6.2 entry cites; the
13.89 of the after row is the kgsl governor settling at 355 MHz instead of 430 once the walk
shortened), walk
<= 3000 and glyphs <= 1200 us, S6 enc <= 2000 us; `replay_us` <= 0.1 ms on S1 / S6; `upload_us` <=
0.2 ms except Pixel 4a S1 (216); warm `first_encode_done` to `first_frame_presented` <= 25 ms median
on both phones; `am start -W` not worse within noise (131 vs 128, inside that pass's 126-141
spread); no scene worse on p50 and enc total where a not-worse bar was set (p95 moved within its
spread on Pixel 4a S6 and Pixel 4 S3, and the two S5 rows that sit beside their befores — Pixel 4
p50 6.32 to 6.41, OnePlus 9 p95 7.01 against 6.99 — are within noise). Not met: Pixel 4 S1 `gpu_main` <= 5.0 ms (CPU-bound) and enc
<= 3000 us; every iPhone SE target (`upload` dominates there); S4 `submit - gpu_main` <= 1.0 ms (set for the Pixel 4
and not evaluated there after 2acaf544; on the phones that were measured: inline path at b6e5a665
Pixel 4a 1.39 / OnePlus 9 3.50, split path at 78fb0a4d 1.66 / 3.41); Pixel 4a S4 p95 <= 12 (12.42); OnePlus 9 S4 p50 <= 4.3 / p95 <= 5.0. Every kill
switch's off-arm reproduced its before rows within noise, except the paint-class all-off arm on the
Pixel 4a (16-17 % below: the atlas tier rule and the clear skip have no switch). Owed: Pixel 4 rows
after c61ee81a (S4 / S5 after 2acaf544), iPhone SE rows after c61ee81a, split-path S2 / S3 / S6
re-captures on the Pixel 4a and OnePlus 9 after c61ee81a, and the Pixel 5 / Xiaomi 12 matrix.

## Dev loop

Measured baseline: default-config incremental `cargo build` medians 0.89s; edit-to-first-frame medians ~263ms once built. An alternate linker and `cranelift` both measured worse, so **no fast-dev template recipe ships** — re-run `scripts/devloop-measure.sh` if that changes. Measured 2026-09-25 (i5-12600, Linux): a watched relaunch's incremental build took 1.29s, matching the baseline above (`frust run --watch`, TUI `R`/`W`; see [DEVELOPMENT.md](DEVELOPMENT.md) § Dev Loop).

## Release artifacts

**Android release minification.** A generated app's `release`/`profile` Gradle build
type runs R8 (`isMinifyEnabled`/`isShrinkResources = true`) against `proguard-rules.pro` (keeping
the Frust JNI surface and the vendored `accesskit_android` delegate); `--debug` is unaffected. NDK
r27+ already 16KB-aligns `.so` LOAD segments, so Android 15's page-size rule needs no linker-flag
change. **Release-lean mode.** `--release` also compiles out all `perf-trace` instrumentation and
enables the app's `lean` feature (`log/release_max_level_warn`), matching Flutter's release
log-level parity; `scripts/release-lean-check.sh` is the manual strings-absence gate. **The no-`db`
`frust_bench` size-matrix APK is a debug-signed measurement artifact, not a release** — built via
cargo-ndk + `gradlew assembleRelease`, not `frust build apk --release` (which refuses without
signing material); never distribute what `benchmarks/harness/app_size.sh` measures.

## Benchmarks

`benchmarks/` is a paired Frust-vs-Flutter measurement suite, out-of-tree from the crate workspace:
`frust_bench/` (standalone Cargo package, gate from its own directory like `examples/huddle`) and
`flutter_bench/` (Flutter SDK 3.44.2 stable) implement the same scenarios — eight timed UI scenarios
(S1–S8) plus two DB op-latency scenarios (D1/D2) — via `harness/`'s shared scripts. The exploratory,
single-sided terminal-grid and IME-capability probes live in `examples/playground` instead.

```bash
./benchmarks/harness/run.sh <scenario> --app frust|flutter --device <serial>
```

`benchmarks/PROTOCOL.md` is the published methodology; `benchmarks/RESULTS.md` is the filled record
of actual device runs (never placeholder/projected numbers). **macOS `mktemp` caveat.**
`harness/run.sh`'s `mktemp` only expands correctly under GNU mktemp — alias it to `gmktemp` (`brew
install coreutils`) until fixed.
