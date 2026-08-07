# Learning Frust's Rendering Pipeline — By Doing

A curated, hands-on curriculum for understanding how Frust turns a `View`
into pixels, built entirely against **this repository's actual code**. Every
chapter is a lab: a concept, the exact files where it lives here, and a
runnable experiment. Zero chapters begin with "first, read this 40-page
paper."

This is the project-specialized counterpart to the generic external roadmap
(Giesen's pipeline series, Raph Levien's blog, etc.). Where a chapter touches
theory, it links the *one* external source that pays off — after you've seen
the mechanism working in this repo.

## How to use this

1. Do the labs in order the first time; each builds vocabulary the next uses.
2. Actually run the experiments. The whole premise is that
   `FRUST_TRACE=1 cargo run` teaches you more than a chapter of prose.
3. Line numbers were verified against the repo at the time of writing
   (2026-07-22, HEAD `0f07565`; re-verified 2026-08-06, HEAD `e2c95ff`) and
   **will drift**. Symbols won't — search for the named function/type if a
   line number misses.
4. Everything desktop-runnable works on this machine today. Device labs
   (chapter 7) need a physical Android/iOS device — remember the iOS
   Simulator can't render under the vello 0.9 pin
   (`docs/DEVELOPMENT.md` → Known Issues).

## Prerequisite (one-time)

```bash
cargo build --workspace --locked && cargo test --workspace   # the repo's verify gate
(cd benchmarks/frust_bench && cargo run)                      # your primary lab vehicle
```

If `frust_bench` opens a window full of physics-driven gradient bubbles (the
S1 scenario, which runs by default) with an FPS meter, you're ready.

## The pipeline, end to end (the map you'll fill in)

```
app_logic(&mut State) -> View            (your code, every frame)
   │  rebuild: diff view vs retained Widget tree        ── ch. 3
   ▼
RenderRoot::layout   (BoxConstraints down, Size up)     ── ch. 2, 3
   ▼
RenderRoot::paint    (widgets → PaintScene commands)    ── ch. 2
   ▼
frust_scene::Scene   (renderer-agnostic display list)   ── ch. 1
   ▼
frust_render::encode_scene  (Command → vello::Scene)    ── ch. 4
   ▼
vello Renderer::render_to_texture  (compute shaders)    ── ch. 5
   ▼
SurfaceRenderer::present  (blit → swapchain → screen)   ── ch. 4
```

Text takes a side road (shape → cache → GlyphRun → draw_glyphs): chapter 6.
On Android/iOS, a frame gate decides whether any of this runs at all:
chapter 7. Measuring all of it is chapter 8.

## Chapters

| # | Lab | You will |
|---|-----|----------|
| [1](01-scene-display-list.md) | The display list | Build a `Scene` by hand in a unit test; read every `Command` variant |
| [2](02-widget-paint.md) | Paint your own pixels | Modify the S1 bubble-chart's canvas widget; write a custom widget; assert paint output GPU-free |
| [3](03-frame-loop.md) | Anatomy of a frame | Trace one desktop frame from `RedrawRequested` to `present()`, matching `FRUST_TRACE` output line-by-line to code |
| [4](04-encode-present.md) | Scene → GPU | Read the `Command`→vello translation; flip the CPU render tier; watch surface lifecycle states |
| [5](05-vello-internals.md) | Inside vello 0.9 | Read the actual WGSL compute stages from your local cargo registry; map them to the sort-middle architecture |
| [6](06-text-pipeline.md) | Text: string → glyphs | Run the shaping tests; measure the shape cache doing its job |
| [7](07-mobile-frame-gate.md) | Mobile loops & the frame gate | Flip the gate/resampler kill switches on a device and watch the cost |
| [8](08-measure-everything.md) | Measurement lab | Drive raw per-frame traces, startup spans, and the S1–S8 benchmark harness |
| [9](09-native-widgets-pipeline.md) | Native widgets | See how OS sibling views composite with the GPU surface (and how RN/Flutter do it) |

## Mapping to the external roadmap (when you want the theory)

| After chapter | The one external read that will now click |
|---|---|
| 1–2 | Nothing yet — stay hands-on |
| 3 | Fabian Giesen, "A trip through the Graphics Pipeline" parts 1–2 (what a swapchain/present actually is) |
| 4–5 | Raph Levien, "A sort-middle architecture for 2D graphics" + "Fast 2D rendering on GPU" — you'll recognize every stage from the WGSL filenames you just read |
| 5 | Levien's prefix-sum posts (the `pathtag_scan`/`pathtag_reduce` shaders you saw ARE decoupled look-back prefix sums) |
| 6 | Pomax's Bézier primer §whatever you got curious about; the Slug/SDF material only if you outgrow atlas-free glyph curiosity |
| 8 | Giesen part 13 (compute shaders) + the Cornell GPU-architecture pages, now that you have your own numbers to explain |

### Verified video watchlist

Every URL below was verified to exist — title and channel checked against
YouTube's own oEmbed metadata (or the hosting archive) on 2026-07-22.
Ordered by when they pay off in this curriculum.

**This stack (Vello / wgpu) — after chapters 4–5:**

| Video | Who / where | Link |
|---|---|---|
| *Vello: high performance 2D graphics* — the single best overview of the renderer under `frust-render` | Raph Levien, RustLab 2023 (RustLab Conference channel) | [youtube.com/watch?v=mmW_RbTyj8c](https://www.youtube.com/watch?v=mmW_RbTyj8c) |
| *Faster, easier 2D vector rendering* — newer Levien talk; covers where vello is heading (the sparse-strips work you met in lab 5's caveat) | Raph Levien, RustNL (RustNL channel) | [youtube.com/watch?v=_sv8K190Zps](https://www.youtube.com/watch?v=_sv8K190Zps) |
| *Compute Shader 101* — Levien teaching exactly the compute-shader mental model vello is built on; companion repo `github.com/googlefonts/compute-shader-101` | Raph Levien (personal channel) | [youtube.com/watch?v=DZRn_jNZjbw](https://www.youtube.com/watch?v=DZRn_jNZjbw) |
| *Building WebGPU with Rust* — from wgpu's then-lead; what sits under vello | Dzmitry Malyshau, FOSDEM 2020 (video on the talk page) | [archive.fosdem.org/2020/…/rust_webgpu](https://archive.fosdem.org/2020/schedule/event/rust_webgpu/) |

**University lectures — alongside chapters 3–5, deeper for chapter 8:**

| Video | Who / where | Link |
|---|---|---|
| *Introduction to Computer Graphics* (CS 4600, full course) — the "GPU Pipeline" lecture is the traditional-rasterization gap-filler this guide skips past | Cem Yuksel, University of Utah, Fall 2020 | [playlist](https://www.youtube.com/playlist?list=PLplnkTzzqsZTfYh4UbhLGpI5kGd5oW_Hh) · [GPU Pipeline lecture](https://www.youtube.com/watch?v=UzlnprHSbUw) |
| *Parallel Computer Architecture and Programming* (15-418, Spring 2016) — Lecture 5 is the canonical GPU-architecture/SIMT lecture; explains why warp divergence matters in `fine.wgsl` | Kayvon Fatahalian, CMU | [playlist](https://www.youtube.com/playlist?list=PLpIxOj-HnDsO4Atvrp86c-4La9Mq3kMQZ) |
| *Intro to Parallel Programming* (CS344) — CUDA-era but the scan/prefix-sum units are exactly vello's `pathtag_scan` math. Caveat: the linked playlist is Unit 1 only; the scan lectures live as separate videos on the same channel (e.g. [Segmented Scan](https://www.youtube.com/watch?v=Yp2eDz6dvNA)) | John Owens (UC Davis) & David Luebke (NVIDIA), Udacity | [Unit 1 playlist](https://www.youtube.com/playlist?list=PLAwxTw4SYaPm0z11jGTXRF7RuEEAgsIwH) |

**Curves & text — alongside chapter 6:**

| Video | Who / where | Link |
|---|---|---|
| *The Beauty of Bézier Curves* — the best 25 minutes on the math inside every `BezPath` you painted | Freya Holmér, 2021 | [youtube.com/watch?v=aVwxzDHniEw](https://www.youtube.com/watch?v=aVwxzDHniEw) |
| *The Continuity of Splines* — the sequel; watch when curve joins/strokes start mattering to you | Freya Holmér, 2022 | [youtube.com/watch?v=jvPPXbo87ds](https://www.youtube.com/watch?v=jvPPXbo87ds) |
| *The Art of Text (rendering)* — glyphs, hinting, kerning, shaping; the context around what Parley does for `frust-text` | 39C3 (Dec 2025), media.ccc.de channel | [youtube.com/watch?v=XTgIJUwmz0Q](https://www.youtube.com/watch?v=XTgIJUwmz0Q) |
| *GLyphy: a GPU-accelerated text rendering engine* — HarfBuzz's author on SDF glyph rendering (the road vello didn't take) | Behdad Esfahbod, Libre Graphics Meeting 2012 | [youtube.com/watch?v=hheWzVA6llc](https://www.youtube.com/watch?v=hheWzVA6llc) |

**The other engines (comparison context) — whenever curiosity strikes:**

| Video | Who / where | Link |
|---|---|---|
| *Introducing Impeller — Flutter's new rendering engine* — the engine bubblebench's upstream repro (flutter#180958) runs on | Flutter channel (@flutterdev), around Google I/O 2023 | [youtube.com/watch?v=vd5NqS01rlA](https://www.youtube.com/watch?v=vd5NqS01rlA) |
| *Life of a Pixel* (Chrome University 2020) — Chrome's whole path through Skia to the compositor; the classic "how a browser renders" talk | BlinkOn channel | [youtube.com/watch?v=PwYxv-43iM4](https://www.youtube.com/watch?v=PwYxv-43iM4) |
| *Flutter's Rendering Pipeline* — 2016 and still the clearest constraints-down/sizes-up walkthrough; Frust's layout model (lab 2) is this model | Adam Barth, Google TechTalks | [youtube.com/watch?v=UUfXWzp0-DU](https://www.youtube.com/watch?v=UUfXWzp0-DU) |

Not found despite looking: a recording of Raph Levien's UCSC "I want a good
parallel computer" colloquium (the blog post stands alone), Eric Lengyel's
Slug talk (slides only, `terathon.com/i3d2018_lengyel.pdf`), and any
dedicated Skia Graphite talk — the Chromium blog post remains the source
there.
