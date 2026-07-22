# Lab 5 — Inside vello 0.9 (reading the real thing, locally)

**Concept:** Below `render_to_texture` lies vello's all-compute pipeline: a
chain of ~20 WGSL compute dispatches that flatten curves, sort work into
16×16-pixel tiles, and rasterize per tile. You don't need the blog posts
first — the pinned source, *including every shader*, is already extracted on
this machine. Read it like any other dependency.

## Where it lives (on this machine)

Registry root: `/Users/ed/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`

| Crate | Path | What's in it |
|---|---|---|
| `vello-0.9.0` | `<registry>/vello-0.9.0/` | `src/scene.rs` (the API you feed), `src/lib.rs` (`Renderer`, `RenderParams`) |
| `vello_shaders-0.9.0` | `<registry>/vello_shaders-0.9.0/shader/` | **33 WGSL files** — the entire GPU pipeline, human-readable |
| `vello_encoding-0.9.0` | `<registry>/vello_encoding-0.9.0/` | The binary scene format (`Encoding`) that `vello::Scene` accumulates |
| `wgpu-29.0.4` | `<registry>/wgpu-29.0.4/` | The GPU abstraction underneath |

(If a `cargo clean`-adjacent operation ever removes these, any workspace
build re-extracts them. Your editor can open them read-only; `rust-analyzer`
go-to-definition from `frust-render` lands there too.)

Verified ground truth worth knowing: `vello::Scene` is literally
`struct Scene { encoding: Encoding }` (`vello-0.9.0/src/scene.rs` ≈44–49) —
when frust's `SceneSink` calls `fill()`, vello appends to a compact byte
stream (path tags, transforms, draw tags), *not* an object tree. The GPU
pipeline's input is that stream.

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
your bubblebench experiment 2.1 was stressing).

## Experiments

### 5.1 — Follow one fill into the stream

Open `vello-0.9.0/src/scene.rs`, find `pub fn fill(..)` and follow it into
`encoding.encode_*` calls; skim `vello_encoding-0.9.0/src/encoding.rs` to see
what actually got appended (tags + points, no objects). Connect this to
chapter 4: frust's `encode_us` timing covers *appending to this stream* plus
dispatching the pipeline above.

### 5.2 — Read `fine.wgsl` with your chapter-2 experiment in mind

In experiment 2.1 you saw radial gradients cost more than solid fills.
`fine.wgsl` shows you *why*: find the per-pixel brush evaluation and compare
the solid-color path to the gradient path (per-pixel math + texture/ramp
sampling). This is the "learn theory after you've measured it" moment this
guide is built on.

### 5.3 — Why the iOS Simulator can't run this

`docs/DEVELOPMENT.md` (Known Issues) records that vello 0.9 requires
`DownlevelFlags::INDIRECT_EXECUTION` — and chapter 4 showed you the probe
(`tier.rs::GPU_REQUIRED_DOWNLEVEL_FLAGS`). Now find the *reason*: grep
`vello-0.9.0/src/` for `indirect` — the pipeline sizes several dispatches on
the GPU (`*_setup.wgsl` stages writing indirect dispatch args), so the
driver must support GPU-driven dispatch. A pipeline this dynamic can't
pre-declare workgroup counts from the CPU. That's not trivia; it's the
defining trait of an all-compute renderer.

### 5.4 — (Optional) Capture a real frame

On this Mac, Xcode's Metal debugger can capture bubblebench:
`cargo build` the example, then Xcode → Debug → Debug Executable… → pick
`target/debug/bubblebench`, enable GPU Frame Capture (Metal), run, capture a
frame — you'll see the compute dispatch chain above by name, with timings
per stage. (On Linux/Android, RenderDoc plays the same role over Vulkan.)
Optional because setup friction is real — but nothing cements the table
above like seeing your own bubbles pass through it.

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

Caveat for the future: upstream vello is mid-transition to a *sparse strips*
architecture (`vello_cpu`/`vello_hybrid` share it — you already ran
`vello_cpu` in lab 4.2). When this repo's pin eventually moves, the stage
table above changes; the frust-side seams (chapters 1–4) are designed not to.
