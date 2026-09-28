# Lab 5 — Inside vello 0.9 (historical background — frust no longer depends on it)

> **This chapter is external theory, not a description of frust's current renderer.** As of Phase
> 8, `vello`/`vello_shaders`/`vello_encoding` were fully deleted from this workspace — `wgpu` is
> now a frust-owned pin with nothing above it — and `frust-engine` (a sparse-strips renderer, no
> compute pass at all) is the only renderer `frust-render` contains. See lab 4 for the current
> architecture. This chapter stays because vello's all-compute design is still the clearest local
> worked example of the GPU-pipeline theory chapters 4-5 used to teach; read it as background on a
> *different* rendering architecture, not on this one.

**Concept:** Below `render_to_texture` lies vello's all-compute pipeline: a
chain of ~20 WGSL compute dispatches that flatten curves, sort work into
16×16-pixel tiles, and rasterize per tile. You don't need the blog posts
first — the pinned source, *including every shader*, was extracted on this
machine by an earlier, unrelated build. Read it like any other dependency.

## Where it lives (on this machine, not in this workspace's lockfile)

Registry root: `~/.cargo/registry/src/<index-hash>/` (the hash segment, e.g.
`index.crates.io-1949cf8c6b5b557f`, is a per-machine registry-index id — yours will differ)

| Crate | Path | What's in it |
|---|---|---|
| `vello-0.9.0` | `<registry>/vello-0.9.0/` | `src/scene.rs` (the API a vello caller feeds), `src/lib.rs` (`Renderer`, `RenderParams`) |
| `vello_shaders-0.9.0` | `<registry>/vello_shaders-0.9.0/shader/` | **33 WGSL files** — the entire GPU pipeline, human-readable |
| `vello_encoding-0.9.0` | `<registry>/vello_encoding-0.9.0/` | The binary scene format (`Encoding`) that `vello::Scene` accumulates |
| `wgpu-29.0.4` | `<registry>/wgpu-29.0.4/` | The GPU abstraction underneath (this workspace is now pinned to 30.0.1 instead — see [RENDER_DEVELOPMENT.md](../RENDER_DEVELOPMENT.md)) |

These are leftover from before Phase 8 — nothing in this workspace's `Cargo.lock` references
`vello` any more, so a `cargo clean`-adjacent operation that prunes the registry cache will NOT
bring them back on the next `frust` build. Fetch them by hand (`cargo add vello@0.9.0` in a
scratch directory, or `cargo vendor`) if they are ever missing and you still want to read them.

Verified ground truth worth knowing: `vello::Scene` is literally
`struct Scene { encoding: Encoding }` (`vello-0.9.0/src/scene.rs` ≈44–49) —
a vello caller's `fill()` call appends to a compact byte
stream (path tags, transforms, draw tags), *not* an object tree. The GPU
pipeline's input is that stream. (`frust-engine`'s own scene compiler, by
contrast, walks `frust_scene::Scene::commands` directly into GPU strip data —
no intermediate byte-stream encoding — see lab 4.)

## The pipeline, by shader filename

The stage shaders in `vello_shaders-0.9.0/shader/` (utilities live in
`shared/`):

| Stage | Shaders | What it does |
|---|---|---|
| Path preprocessing | `pathtag_reduce`, `pathtag_reduce2`, `pathtag_scan`, `pathtag_scan1` | **Prefix sums** over the encoded path-tag stream to locate every path/segment in parallel |
| Bounding boxes | `bbox_clear`, `draw_reduce`, `draw_leaf` | Per-draw metadata + bboxes |
| Flattening | `flatten` | Béziers → line segments (GPU-side; also handles stroke expansion in 0.9) |
| Clipping | `clip_reduce`, `clip_leaf` | Resolve the clip stack (a parallel *stack monoid*) |
| Binning | `binning` | Sort draw commands into coarse spatial bins — the "sort-middle" step |
| Tiling | `tile_alloc`, `path_count(_setup)`, `path_tiling(_setup)`, `backdrop`, `backdrop_dyn` | Allocate per-tile segment lists, compute winding backdrops |
| Coarse raster | `coarse` | Per 16×16 tile: build a *per-tile command list* (PTCL) — "a tiny program for each tile" |
| Fine raster | `fine` | Interpret each tile's PTCL: compute exact coverage, evaluate brushes/gradients, blend, write pixels |

This table *is* Raph Levien's "sort-middle architecture" post, in filenames.
Read in this order: `flatten.wgsl` (most familiar math) → `binning.wgsl` →
`coarse.wgsl` → `fine.wgsl` (the payoff: find the gradient-evaluation code
your chapter-2 bubble-chart experiment (2.1) was stressing).

## Experiments

### 5.1 — Follow one fill into the stream

Open `vello-0.9.0/src/scene.rs`, find `pub fn fill(..)` and follow it into
`encoding.encode_*` calls; skim `vello_encoding-0.9.0/src/encoding.rs` to see
what actually got appended (tags + points, no objects). Contrast this with
lab 4's `SceneCompiler::compile`, which has no such intermediate stream — it
walks the scene straight into `GpuStrip`/`GpuEncodedPaint` records.

### 5.2 — Read `fine.wgsl` with your chapter-2 experiment in mind

In experiment 2.1 you saw radial gradients cost more than solid fills.
`fine.wgsl` shows you *why*: find the per-pixel brush evaluation and compare
the solid-color path to the gradient path (per-pixel math + texture/ramp
sampling). This is the "learn theory after you've measured it" moment this
guide is built on.

### 5.3 — Why vello 0.9 couldn't run on the iOS Simulator (and why frust's engine can)

vello 0.9 required `DownlevelFlags::INDIRECT_EXECUTION`, which the Simulator's Apple2 Metal
feature family never exposes. Find the *reason*: grep `vello-0.9.0/src/` for `indirect` — the
pipeline sizes several dispatches on the GPU (`*_setup.wgsl` stages writing indirect dispatch
args), so the driver must support GPU-driven dispatch. A pipeline this dynamic can't pre-declare
workgroup counts from the CPU. That's not trivia; it's the defining trait of an all-compute
renderer — and exactly the trait `frust-engine` (lab 4) does not share: no compute pass at all, so
`ENGINE_REQUIRED_DOWNLEVEL_FLAGS` (`crates/frust-render/src/tier.rs`) is empty and the Simulator
renders correctly under it today.

### 5.4 — (Optional) Capture a real frame

On this Mac, Xcode's Metal debugger can capture the S1 bubble scenario:
`cargo build` in `benchmarks/frust_bench`, then Xcode → Debug → Debug
Executable… → pick `target/debug/frustbench`, enable GPU Frame Capture
(Metal), run, capture a frame — you'll see the compute dispatch chain above
by name, with timings per stage. (On Linux/Android, RenderDoc plays the same
role over Vulkan.) Optional because setup friction is real — but nothing
cements the table above like seeing your own bubbles pass through it.

## When the theory will land (and only now)

You've now *seen*: a byte-stream scene encoding, prefix-sum shaders, a
binning stage, per-tile command lists, and a fine-raster interpreter. Read,
in this order, and each will feel like documentation of code you know:

1. Raph Levien, "A sort-middle architecture for 2D graphics" (2020-06-12)
2. "Fast 2D rendering on GPU" (2020-06-13)
3. "Prefix sum on portable compute shaders" (2021-11-17) — then reopen
   `pathtag_scan.wgsl`
4. "GPU-friendly Stroke Expansion" (2024) — then reopen `flatten.wgsl`
5. "I want a good parallel computer" (2025) — the retrospective; his "vello
   is a compiler producing a bytecode program per 16×16 tile" line is
   literally `coarse.wgsl` → `fine.wgsl`

This already happened here: upstream vello's transition to a *sparse strips* architecture
(`vello_cpu`, and the vendored `vello_common`/`glifo` core `frust-engine` is built on) is exactly
what replaced the all-compute pipeline this chapter describes. The stage table above no longer
describes what runs in this repo — lab 4's compiler/scheduler/pipeline seam is the sparse-strips
equivalent, and it is what the frust-side seams (chapters 1–4) were designed to stay stable
across.
