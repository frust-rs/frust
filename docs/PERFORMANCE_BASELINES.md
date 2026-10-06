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
| _pending (d1-03)_ | main f79b739c vs 79686b68 | i5-12600 Linux | material3-demo | _pending (d1-03)_ | _pending (d1-03)_ | – |
| _pending (d1-03)_ | main f79b739c vs 79686b68 | i5-12600 Linux | frust-gallery + material3-demo (`cargo build --timings`) | – | – | _pending (d1-03)_ |

The unstripped figure is within 328 B of the stripped one because `strip = "symbols"` already strips
the release profile. Each pending row is filled when d1-03 measures it.

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
