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
windows over 180 untimed frames, plus the compile spans). Later cards append rows.

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
launch loads that same 328 B. S1's atlas on both phones (re-captured at 9e510235 with `glyph_misses` on the
production path, 6x30 s, first two runs discarded): every drawn glyph is atlas-resident (`glyph_draws ==
atlas_glyphs`: 466 on the Pixel 4a, 458 on the Pixel 4), yet per 60-frame window the Pixel 4a replays on 37
frames with 66 misses and 69 evictions and the Pixel 4 on 40 frames with 81 misses and 93 evictions, and nothing
is refused. Replay is gated on misses by construction, so the before-number to beat is the miss count; in steady
state the misses roughly equal the evictions (1.8-2.1 per replaying frame). The working hypothesis for the
replay-gating card is that evicted entries are re-rasterised while still in use (age-based eviction of the
4-bucket subpixel variants of moving text fits the same counters), which needs per-key evidence — a miss on a key
evicted earlier while its glyph kept being drawn — before it is treated as the lever. Pixel 4 S1 re-captured cool
(32.7 C start): p50 11.66 / p95 15.44 ms, drifting to 35 C by run 6. Thread sample during S1: main,
`RenderThread` and both `frust-reactive` threads at nice -10, `frust-render` at nice -4.

### After Phase 1

Re-captured 2026-10-14 from `feature/frust-fast-p1` (2acaf544: paint-class programs, one-pass opening, atlas
tier rule + clear skip, hardware image sampler) with the same protocol (default build, 12x30 s, first two runs
discarded, same fairness warnings, clocks unpinned); every scenario started below 34.0 C (Pixel 4 S1 / S5 / S6 /
S4 at 31.5 / 33.8 / 34.0 / 33.8 C, the Pixel 4a within 31.5-34.0 C). Pixel 4a `kgsl-3d0` sampled at 1 Hz,
median governor clock / busy Mcyc/s: S1 430 MHz / 347 (81 % busy), S4 180 / 34, S5 355 / 295, S6 267 / 150.

Pixel 4a:

| Scenario | total p50 / p95 | submit p50 | gpu_main p50 / p95 | walk / glyphs / upload / replay / instances | draws / strips / alphas |
|---|---|---|---|---|---|
| S1 | 16.61 / 18.39 | 10.60 | 12.75 / 12.99 | 6813 / 3899 / 585 / 1233 / 234 | 536 / 14,783 / 494,208 |
| S4 | 11.97 / 12.79 | 4.25 | 2.69 / 2.75 | 697 / 318 / 313 / 0.8 / 52 | 46 / 1,056 / 26,832 |
| S5 | 14.19 / 18.48 | 6.15 | 13.30 / 13.71 | 2459 / 167 / 732 / 1.0 / 97 | 31 / 3,050 / 84,512 |
| S6 | 13.78 / 15.23 | 6.55 | 8.79 / 10.75 | 2972 / 2704 / 446 / 0.8 / 249 | 512 / 8,925 / 216,336 |

Pixel 4 (S1 `gpu_main` held at 8.11-8.12 in every captured run, discarded ones included, while `submit`
drifted 10.5 to 14.1 ms as the phone reached 36.5 C from run 9):

| Scenario | total p50 / p95 | submit p50 | gpu_main p50 / p95 | walk / glyphs / upload / replay / instances | draws / strips / alphas |
|---|---|---|---|---|---|
| S1 | 11.15 / 14.70 | 10.66 | 8.12 / 8.40 | 8238 / 6593 / 315 / 864 / 139 | 525 / 14,077 / 474,272 |
| S4 | 3.78 / 4.33 | 3.25 | 0.90 / 0.93 | 654 / 320 / 248 / 0.4 / 46 | 45 / 1,025 / 27,696 |
| S5 | 6.41 / 7.17 | 4.78 | 5.52 / 5.59 | 1849 / 140 / 510 / 0.4 / 85 | 30 / 3,001 / 92,448 |
| S6 | 6.90 / 7.52 | 5.61 | 3.65 / 3.69 | 2642 / 2444 / 348 / 0.3 / 211 | 508 / 8,918 / 224,184 |

Kill-switch off-arm (`FRUST_ENGINE_UBER_PROGRAM=1 FRUST_ENGINE_NO_PASS_MERGE=1 FRUST_ENGINE_IMAGE_SAMPLER=0`,
same APK recipe otherwise; before-row `gpu_main` p50 in the last column):

| Phone / scenario | total p50 / p95 | submit p50 | gpu_main p50 / p95 | 4a clock / busy | before |
|---|---|---|---|---|---|
| Pixel 4a S1 | 32.57 / 35.99 | 14.10 | 27.10 / 27.50 | 700 MHz / 698 (100 %) | 32.70 |
| Pixel 4a S5 | 17.84 / 37.02 | 5.68 | 21.97 / 22.42 | 700 MHz / 699 (100 %) | 26.27 |
| Pixel 4 S1 | 14.01 / 16.04 | 13.56 | 10.70 / 10.93 | - | 11.15 |
| Pixel 4 S5 | 12.02 / 12.86 | 4.76 | 8.55 / 8.83 | - | 8.54 |

The off-arm reproduces the Pixel 4 before-rows within noise (S1 10.70 vs 11.15, both warm: runs 1-4 sat at
9.16-9.18 before the throttle from run 5; S5 8.55 vs 8.54) but not the Pixel 4a ones: 27.10 vs 32.70 and
21.97 vs 26.27 (16-17 % below) with the governor at 700 MHz and 100 % busy. The three switches account for
27.10 to 12.75 and 21.97 to 13.30; the unswitched atlas tier rule + clear skip, and the unsampled before-clock,
are the candidates for the rest. Acceptance: Pixel 4a S1 `gpu_main` <= 16 ms MET at 12.75 (before 32.70; S1 total
p50 16.61 sits at the 60 Hz budget, p95 18.39 still over it); Pixel 4 S1 `gpu_main` <= 5.0 ms NOT MET at 8.12 (before
11.15) — that frame is CPU-bound, `submit` p50 10.66 ms with walk 8.2 ms against the 11.1 ms 90 Hz budget, and the
Pixel 4's kgsl is not
readable, so the only clock evidence is the Pixel 4a's 347 vs 698 Mcyc/s; no S4 / S6 regression MET (Pixel 4a S4
7.21 to 2.69, S6 13.02 to 8.79; Pixel 4 S4 2.02 to 0.90, S6 7.32 to 3.65). S1 atlas counters moved with the
frame rate: Pixel 4a replays 50 / misses 126 / evictions 133 per window (before 37 / 66 / 69), Pixel 4 41 / 86 / 97.

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
