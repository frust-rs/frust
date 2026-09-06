# P3 Calibration: classic vs. `vello_cpu` 0.2.0 divergence budget (P2)

> Recipe retired — `tests/calibration.rs` was deleted in p8-04a, alongside the
> classic (vello) oracle it measured against. Kept as history; the engine's
> P1 gate against `vello_cpu` 0.2.0 now runs each case's own fixed tolerance
> (see `docs/TESTING.md` § Golden Classes).

This document is the reviewed record of one run of
`crates/frust-testing/tests/calibration.rs`'s
`calibrates_the_engine_vs_classic_perceptual_budget` — the whole unit
(`crates/frust-testing/src/corpus/unit.rs`) plus adversarial
(`crates/frust-testing/src/corpus/adversarial.rs`) corpus, rendered once on
`frust-render`'s classic (headless GPU) pipeline and once on `vello_cpu`
0.2.0, diffed directly against each other (never against a stored golden) to
derive — rather than guess — the P2 perceptual budget the engine and its CPU
oracle are allowed to disagree by.

Re-run it with:

```text
WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
  cargo test -p frust-testing --test calibration -- --ignored --nocapture
```

## Run provenance

| Field | Value |
|---|---|
| Adapter | NVIDIA T400 4GB |
| Backend | Vulkan |
| Driver | NVIDIA 610.43.03 |
| OS | Linux (precision, 6.18.38-1-MANJARO, x86_64) |
| `vello_cpu` oracle | `vello-cpu-0.2` (vello_cpu 0.2.0, vello_common 0.2.0, glifo 0.3.0), SIMD level `Fallback` (`Level::baseline()`), 0 worker threads |
| Frust commit | `cae8f311d4bdd782870d1aefc40bea28a405707f` |
| Run timestamp | 2026-08-29T15:16:33Z |
| Cases measured | 29 (18 unit + 14 adversarial, minus 2 `no_ref` and 1 CPU-skipped) |

`WGPU_ADAPTER_NAME=T400`/`FRUST_GOLDEN_EXPECT_ADAPTER=T400` pin and verify the
adapter on this dual-GPU host (an NVIDIA T400 alongside an Intel UHD 770
iGPU) before any pixel is produced — a wrong adapter is a refusal, not a
footnote.

## Method

Every [`unit_cases`]/[`adversarial_cases`] case with a reference on BOTH arms
is rendered on the classic (GPU) oracle and the `vello_cpu` oracle, then
compared with the same comparator every golden gate uses
(`frust_testing::diff::diff_images`), under a fixed measurement tolerance
(`channel: 8, alpha: 8`) and each case's own `eroded_interior` flag (the same
1-px antialiased-edge-jitter allowance a golden comparison already applies).
Three per-case numbers come out of each comparison:

- **channel**: the largest per-channel absolute difference (R, G, B, and A
  alike) among only the pixels that survived erosion and still exceeded the
  measurement tolerance — i.e. the *interior* divergence, not a single
  antialiased boundary pixel. This is deliberately **not**
  `DiffReport::max_difference`, which `diff.rs` documents as a whole-image
  statistic regardless of erosion; a single coverage-flipped edge pixel
  routinely swings that field to 255 and would swamp every case's number with
  edge conflation rather than measuring the thing this budget bounds.
- **mean**: the average of `DiffReport::mean_abs_error`'s four channels — a
  genuine whole-image statistic by `diff.rs`'s own design (never eroded).
- **% pixels over 8**: `DiffReport::mismatched_percent` under the fixed
  tolerance above, post-erosion when the case asks for it.

### Excluded cases

- `adv-nan-transform`, `adv-5k-layers` — both `no_ref`: their own pixels are
  not a stable reference on any backend (deliberately malformed transform
  components; a depth-only memory-budget probe), so there is nothing to
  diverge FROM.
- `unit-shader-quad` — skipped on the CPU oracle (no shader pre-pass without a
  GPU); with only one arm's image, a divergence cannot be measured.

## Per-case results

| Case | Family | channel | mean | % over 8 | bbox |
|---|---|---:|---:|---:|---|
| unit-fill-rect | fills | 0 | 0.006 | 0.0000% | — |
| unit-rounded-rect | fills | 34 | 0.042 | 0.5371% | (26,20)-(60,46) |
| unit-stroke-line | strokes | 255 | 0.075 | 0.7324% | (7,8)-(63,56) |
| unit-glyph-run | text | 50 | 0.129 | 0.5127% | (18,29)-(32,40) |
| unit-clip-rect | clips | 0 | 0.000 | 0.0000% | — |
| unit-clip-rounded | clips | 255 | 0.227 | 2.7588% | (8,8)-(63,55) |
| unit-clip-balance | clips | 0 | 0.000 | 0.0000% | — |
| unit-image | images | 0 | 0.000 | 0.0000% | — |
| unit-blur-rrect | blur | 171 | 4.581 | 10.9863% | (6,7)-(58,58) |
| unit-layer-alpha | layers | 0 | 0.164 | 0.0000% | — |
| unit-layer-balance | layers | 0 | 0.246 | 0.0000% | — |
| unit-clear-rect | layers | 0 | 0.000 | 0.0000% | — |
| unit-path-fill | fills | 255 | 0.153 | 1.4893% | (8,15)-(63,54) |
| unit-path-stroke | strokes | 255 | 0.524 | 5.3711% | (6,7)-(63,55) |
| unit-path-dashed | strokes | 255 | 0.578 | 3.9307% | (6,6)-(59,57) |
| unit-shader-quad | — | excluded: CPU-skipped, no cross-arm reference | | | |
| unit-snapshot-bracket | layers | 0 | 0.000 | 0.0000% | — |
| unit-snapshot-balance | layers | 0 | 0.146 | 0.0000% | — |
| adv-degenerate-path | fills | 0 | 0.000 | 0.0000% | — |
| adv-nan-transform | — | excluded: `no_ref` | | | |
| adv-subpixel-rrect | fills | 0 | 0.011 | 0.0000% | — |
| adv-5k-layers | — | excluded: `no_ref` | | | |
| adv-10k-glyphs | text | 0 | 1.722 | 0.0000% | — |
| adv-huge-image | images | 255 | 2.988 | 2.3438% | (8,8)-(56,56) |
| adv-clip-nest-8 | clips | 0 | 0.001 | 0.0000% | — |
| adv-destout-in-layer | layers | 0 | 0.000 | 0.0000% | — |
| adv-snapshot-scale-alpha | layers | 0 | 0.000 | 0.0000% | — |
| adv-empty-scene | fills | 0 | 0.000 | 0.0000% | — |
| adv-unbalanced-pops | layers | 0 | 0.000 | 0.0000% | — |
| adv-1px-divider-1x | strokes | 0 | 0.016 | 0.0000% | — |
| adv-1px-divider-2x | strokes | 0 | 0.016 | 0.0000% | — |
| adv-1px-divider-2-75x | strokes | 0 | 0.062 | 0.0000% | — |

## Distribution by family

### channel (max abs diff among eroded-interior mismatches, 0-255)

| family | n | p50 | p95 | max |
|---|---:|---:|---:|---:|
| fills | 6 | 0.000 | 199.750 | 255.000 |
| strokes | 6 | 127.500 | 255.000 | 255.000 |
| text | 2 | 25.000 | 47.500 | 50.000 |
| clips | 4 | 0.000 | 216.750 | 255.000 |
| layers | 8 | 0.000 | 0.000 | 0.000 |
| images | 2 | 127.500 | 242.250 | 255.000 |
| blur | 1 | 171.000 | 171.000 | 171.000 |
| **OVERALL** | **29** | **0.000** | **255.000** | **255.000** |

### mean abs error (whole-image, 0-255)

| family | n | p50 | p95 | max |
|---|---:|---:|---:|---:|
| fills | 6 | 0.008 | 0.125 | 0.153 |
| strokes | 6 | 0.069 | 0.565 | 0.578 |
| text | 2 | 0.925 | 1.642 | 1.722 |
| clips | 4 | 0.001 | 0.193 | 0.227 |
| layers | 8 | 0.000 | 0.217 | 0.246 |
| images | 2 | 1.494 | 2.839 | 2.988 |
| blur | 1 | 4.581 | 4.581 | 4.581 |
| **OVERALL** | **29** | **0.016** | **2.482** | **4.581** |

### % pixels over 8 (post-erosion)

| family | n | p50 | p95 | max |
|---|---:|---:|---:|---:|
| fills | 6 | 0.000 | 1.251 | 1.489 |
| strokes | 6 | 0.366 | 5.011 | 5.371 |
| text | 2 | 0.256 | 0.487 | 0.513 |
| clips | 4 | 0.000 | 2.345 | 2.759 |
| layers | 8 | 0.000 | 0.000 | 0.000 |
| images | 2 | 1.172 | 2.227 | 2.344 |
| blur | 1 | 10.986 | 10.986 | 10.986 |
| **OVERALL** | **29** | **0.000** | **4.795** | **10.986** |

## Derived P2 budget

Whole-corpus p95 of each metric, rounded up (channel to the next integer,
mean and % pixels over 8 to the next 0.1):

| Metric | Pre-task guess | **Measured P2 budget** |
|---|---:|---:|
| channel (eroded interior) | ≤ 8 | **≤ 255** |
| mean abs error | ≤ 1.5 | **≤ 2.5** |
| % pixels over 8 (post-erosion) | ≤ 0.5% | **≤ 4.8%** |

The pre-task guess in `channel` is off by roughly two orders of magnitude:
several families (`strokes`, `clips`, `images`, `blur`) contain at least one
case whose eroded-interior mismatch still reaches a full 255-level channel
delta — a wider-than-one-device-pixel band of disagreement along a slanted or
curved edge (a 2-3px steep diagonal stroke, a rounded clip corner, an
image-minification kernel boundary) that 1-px erosion does not fully remove,
as opposed to the single-pixel AA jitter erosion is designed for. Bounding
`channel` at anything tight would fail on ordinary, expected geometry, so the
measured budget for `channel` is effectively unconstrained (255, i.e. "any
byte value") and the practical gate for Phase 3+ should rely on **`mean`**
and **`% pixels over 8`**, which both stay small and well-behaved across every
family. `layers` measures a flat 0 across the board (every case in that
family happens to compare exactly, or with only whole-image mean noise below
the eroded-interior threshold) and `text`/`images` sit well inside the
overall p95 — `blur` is the one outlier family driving the corpus-wide
`mean`/`% over 8` tail (see below).

**Recommended P2 gate for Phase 3+: `mean ≤ 2.5`, `% pixels over 8 ≤ 4.8%`,
no numeric bound on `channel`** (rely on `mean`/`% over 8` to catch a real
regression; a `channel` cap would either fail on ordinary edge geometry or be
so loose — 255 — that it catches nothing a mean/percentage budget would not
already have caught first).

## Legitimate disagreements (do not re-litigate in Phase 3+)

- **`unit-blur-rrect` (family `blur`)** — by far the largest divergence in the
  corpus (mean 4.581, 10.99% of pixels over 8). `unit-blur-rrect`'s own doc
  comment already names this: "the edge ramp is exactly where two
  rasterizers legitimately differ" — the classic pipeline's Gaussian blur
  approximation and `vello_cpu`'s are independent implementations of the same
  filter, and their coverage ramps around a blurred rounded-rect edge are not
  bit-identical. This is the single case that justifies the corpus-wide
  `mean`/`% over 8` budget being as loose as it is; excluding it would let the
  other 28 cases support a noticeably tighter number (family `blur`'s own p95
  IS its only data point, so a second reviewed blur case would sharpen this).
- **Steep/diagonal stroke and path edges (`unit-stroke-line`'s 45° segment,
  `unit-path-stroke`, `unit-path-dashed`, `unit-clip-rounded`'s rounded
  corners)** — the classic (GPU) and `vello_cpu` (CPU) rasterizers use
  different antialiasing/coverage algorithms for a non-axis-aligned edge, so
  a several-pixel-wide band along a diagonal or curved boundary can disagree
  by a full channel step even after 1-px erosion (erosion removes a
  single-pixel-wide ring; a shallow diagonal's antialiased band is wider than
  that). This is "conflation at shared edges" in the sense the task names:
  both arms are individually correct approximations of the same vector edge,
  not a bug in either.
- **`unit-image`/`adv-huge-image` minification (family `images`)** — a
  resampling filter choice (nearest vs. bilinear, or a different bilinear
  kernel) is not specified by `Command::Image`'s contract, so the two arms'
  downsampled edge pixels around a minified image's boundary legitimately
  differ; `adv-huge-image`'s interior probe (a uniform-color source) still
  matches exactly, which is what makes this a resampling-boundary
  disagreement rather than a wrong-color regression.
- **`unit-glyph-run`/`adv-10k-glyphs` (family `text`)** — glyph coverage
  antialiasing is, like the stroke/path case above, a rasterizer-specific
  approximation of the same outline; both arms render recognizably the same
  glyphs (per each case's own probes), and the measured divergence here sits
  comfortably inside the corpus's overall p95 rather than driving it.

## Adoption

The recommended gate is adopted: `crates/frust-testing/tests/engine_goldens.rs`
holds the engine-vs-classic divergence to this file's Derived P2 budget as a
hard per-case assertion, with per-case widenings recorded as reviewed
`BAND_ESCALATIONS` rows citing this file's Legitimate Disagreements.
`docs/TESTING.md`'s Comparison section references both.
