# Lab 11 — The engine compiler (Scene → CompiledFrame)

**Concept:** Lab 4 showed you where `frust-engine` sits. This lab opens its first half: the CPU
compiler that turns a `frust_scene::Scene` into *sparse strips* plus the draws, paints and depths
the GPU passes consume. `SceneCompiler::compile` walks the display list once per frame and either
hands back one `CompiledFrame` or refuses the frame whole with an `EngineError`. It is pure CPU
work, so every experiment here runs without a GPU.

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `SceneCompiler::compile` — `Scene` → `Result<CompiledFrame, EngineError>` | `crates/frust-engine/src/compile/mod.rs` | ≈737 |
| `CompileSpans` — the six phases, documented in the order they run | `crates/frust-engine/src/compile/mod.rs` | ≈175–233 |
| `CompiledFrame` (+ `draws()`, `strip_buf()`, `alphas()`) | `crates/frust-engine/src/compile/mod.rs` | ≈290, ≈425 |
| `SceneCompiler::for_caps` — atlas budget + hinting from the device class | `crates/frust-engine/src/compile/mod.rs` | ≈530 |
| `check_geometry` — the up-front finiteness sweep | `crates/frust-engine/src/compile/mod.rs` | ≈2234 |
| `fast_rect` / `is_pixel_aligned` — fast-rectangle admission | `crates/frust-engine/src/compile/mod.rs` | ≈2372 |
| `SceneCompiler::record` — one draw: strips, paint, `depth.advance()` | `crates/frust-engine/src/compile/mod.rs` | ≈1713 |
| `dash_path` — dashes pre-expanded to a plain path (public parity seam) | `crates/frust-engine/src/compile/mod.rs` | ≈2419 |
| `EngineDraw`, `DepthCounter` | `crates/frust-engine/src/compile/draw.rs` | ≈30, ≈70 |
| `GpuStrip`, `alpha_column`, `RECT_STRIP_FLAG` | `crates/frust-engine/src/gpu/strips.rs` | ≈108, ≈98, ≈23 |
| Strip shader header + the depth → z mapping | `crates/frust-engine/shaders/strip.wgsl` | ≈8–15, ≈297 |
| `composite_instance` — who actually emits `RECT_STRIP_FLAG` | `crates/frust-engine/src/renderer.rs` | ≈3132 |
| `EngineError` | `crates/frust-engine/src/error.rs` | ≈9 |
| E17 scan (no `unwrap`, `expect(`, `panic!(` on the frame path) | `crates/frust-engine/tests/structure.rs` | ≈250 |

`StripGenerator`, `StripStorage` and `Strip` are not frust code: they come from `vello_common`, the
sparse-strips rasteriser core the engine keeps (root `Cargo.toml` pins `vello_common` at `=0.2.0`
and `glifo` at `=0.3.0`). There is no `vello` crate in the graph — see lab 5.

## The four ideas

1. **Six phases, one refusal point.** The `CompileSpans` doc lists them in order:
   **validate** (`check_geometry` over *every* command — rects, paths, glyph runs, and the
   clip/layer/snapshot/clear brackets too — plus `check_finite` on each composed transform; the
   frame is refused whole, before anything is recorded) → **prepare** (reset the strip generator,
   clip/group/snapshot stacks and punches; age image residency and `glifo`'s prep cache) →
   **classify** (route every glyph run — atlas or outline — before any is drawn) → **admit** (close
   the glyph atlas's own frame) → **walk** (build the `CompiledFrame` record and a fresh
   `DepthCounter`, then the command walk: strip generation plus paint encoding) → **finish** (close
   open groups, generate hole punches, age the glyph atlas, take the image plan). `for_caps` is
   where the device class enters: desktop-class adapters get hinted glyphs, mobile ones do not.
2. **A sparse strip stores coverage only where it is needed.** `strip.wgsl` says it best:

   > Each strip instance is a horizontal slice of the output made of:
   >
   > 1. a variable-width region of alpha values for semi-transparent rendering, and
   > 2. a solid region for fully opaque areas.
   >
   > The alpha values live in a texture and are sampled during fragment shading,
   > so coverage is stored only where it is actually needed.

   In a `CompiledFrame`, `strips: StripStorage` holds `strips.strips: Vec<Strip>` (each
   `EngineDraw::strip_range` slices it) and `strips.alphas: Vec<u8>` (indexed by each
   `Strip::alpha_idx()`). A `Strip` is just `x`, `y` (a multiple of `Tile::HEIGHT` = 4) and a packed
   alpha index whose bit 31 is `fill_gap`. Its *width* is not stored: `Strip::width_to(next)` reads
   it off the next strip's alpha index, and a run ends in a sentinel strip at `x = 65535`. A
   `fill_gap` strip means "paint solid from the previous strip to me" — zero alpha bytes for the
   interior. Beside the strips: `recorder: CommandRecorder<EngineDraw>` (read via `draws()`, back
   to front), `encoded_paints` (paints too complex to inline), `clears: Vec<ClearPunch>`
   (hole punches kept *outside* `draws()` on purpose), `lut_requests` (gradient ramps to make
   resident), and observational counters — `fast_rect_draws`, `scissor_clips`, `mask_clips`,
   `glyph_draws`, `skipped_images`… Nothing downstream branches on those counters; they exist so
   you can measure the compiler from outside.
3. **The GPU sees 24-byte instances.** The renderer turns strips into `GpuStrip`s: `x`, `y`,
   `width`, `dense_width_or_rect_height` (`u16`s), then `col_idx_or_rect_frac`, `payload`,
   `paint_and_rect_flag`, `depth_index` (`u32`s) — compile-time-asserted to be 24 bytes and
   padding-free. `alpha_column(alpha_idx) = alpha_idx / Tile::HEIGHT` converts a byte index into
   the column the shader addresses; the fragment stage then picks texel `col / 4` and channel
   `col % 4`. The **fast-rect** path is a CPU shortcut, not a different instance kind: when
   `fast_rect` finds the composed transform axis-aligned *and* all four device edges on whole
   pixels, `record` calls `vello_common`'s `generate_filled_rect_fast`, which writes ordinary
   strips directly, skipping flattening and tiling, and `fast_rect_draws` counts it. Bit 31 of
   `paint_and_rect_flag` (`RECT_STRIP_FLAG`) marks a whole-rectangle instance, but today only
   `renderer.rs`'s `composite_instance` (layer composites) emits one — a fast-rect fill does not.
   Dashes never reach the stroker either: `dash_path` expands them to a plain path first, and is
   `pub` so `frust-testing`'s CPU oracle can pin its own copy against it.
4. **Depth is painter's order, and refusal is a value.** `record` stamps each draw with
   `DepthCounter::advance()` — 0 for the back-most draw, +1 per draw, saturating, and a culled draw
   consumes no depth. `strip.wgsl` maps it to z:

   ```wgsl
   // Divide by a power of two so the arithmetic is exact in f32, and by the
   // expected 24 bits of depth-buffer precision.
   let z = 1.0 - f32(instance.depth_index) / f32(1u << 24u);
   ```

   So the back-most draw sits at z = 1.0 and later draws move toward the viewer. How opaque and
   translucent draws split into depth-writing and depth-tested passes is lab 13's story; here, note
   only `EngineDraw::depth` → `GpuStrip::depth_index`. Errors: `EngineError` carries
   `InvalidGeometry` (a NaN/inf rect, radius, path point, stroke width, dash, glyph position),
   `InvalidTransform`, `TargetTooLarge`, and more — every one returned, never panicked. That is
   E17, and `crates/frust-engine/tests/structure.rs` enforces it by grepping the engine's `compile`,
   `gpu` and `text` trees, `renderer.rs` and friends for `unwrap()`, `expect(`, `panic!(` outside
   test modules.

## Experiments

> Run the crate's tests per file, never bare — see the README's note on `proptest_strips`.

### 11.1 — Read the compiler's own contract tests

```bash
cargo test -p frust-engine --test compile_rects_paths
```

Expected: `27 passed`. Open `crates/frust-engine/tests/compile_rects_paths.rs` and read
`a_pixel_aligned_fill_rect_takes_the_fast_rectangle_path` (one draw, `fast_rect_draws == 1`), its
two negatives (rotated; `20.5`-edged), `every_geometry_command_produces_strips` (its path cases take
the general flatten-and-tile route) and `draws_carry_monotone_depths_from_the_back_most` (depths
`[0, 1, 2, 3]`). `cargo test -p frust-engine --test compile_rects_paths fast_rectangle` runs 3.

### 11.2 — Write your own: watch the fast path drop out

Copy the `scene_of` helper from `compile_rects_paths.rs` into a **scratch** file
`lab11_scratch.rs` in `crates/frust-engine/tests/` (delete it when done — never commit it) and add:

```rust
use frust_engine::{EngineError, SceneCompiler};
use frust_scene::{Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::{Brush, color::palette::css::RED};

fn report(label: &str, rect: Rect) {
    let scene = scene_of(|b| b.fill_rect(rect, Brush::Solid(RED)));
    let frame = SceneCompiler::new(200, 200)
        .compile(&scene, Affine::IDENTITY, (200, 200))
        .expect("compiles");
    println!("{label}: strips={} alphas={} fast_rect_draws={}",
        frame.strips.strips.len(), frame.strips.alphas.len(), frame.fast_rect_draws);
}

#[test]
fn lab11() {
    report("aligned", Rect::new(20.0, 20.0, 30.0, 30.0));
    report("half-px", Rect::new(20.5, 20.5, 30.5, 30.5));
    report("aligned 100", Rect::new(20.0, 20.0, 120.0, 120.0));
    report("half-px 100", Rect::new(20.5, 20.5, 120.5, 120.5));
}
```

Run `cargo test -p frust-engine --test lab11_scratch -- --nocapture`. Observed at base
`f1cebf5e` (add a loop printing each strip's `x`, `y`, `alpha_idx()`, `fill_gap()` for the dump):

| Rect | strips | alphas | `fast_rect_draws` |
|---|---|---|---|
| 10×10 at (20, 20) | 6 | 112 | 1 |
| 10×10 at (20.5, 20.5) | 5 | 128 | 0 |
| 100×100 at (20, 20) | 51 | 800 | 1 |
| 100×100 at (20.5, 20.5) | 51 | 1600 | 0 |

In the 100×100 dump, aligned: each 4-pixel row is two 4-column edge strips (32 alpha bytes) with
a `fill_gap` solid span between, then the `x=65535` sentinel. Half-pixel: interior rows look the
same, but the top row (`y=20`) and a new bottom row (`y=120`) each become one 104-column dense
strip (416 bytes). The alpha buffer doubles; the strip count does not move.

### 11.3 — Refusal is a value

Add to the scratch test:

```rust
let scene = scene_of(|b| b.fill_rect(Rect::new(f64::NAN, 20.0, 30.0, 30.0), Brush::Solid(RED)));
match SceneCompiler::new(200, 200).compile(&scene, Affine::IDENTITY, (200, 200)) {
    Err(EngineError::InvalidGeometry) => println!("NaN rect: refused, InvalidGeometry"),
    other => println!("unexpected: {other:?}"),
}
```

Observed: `NaN rect: refused, InvalidGeometry` (a NaN *transform* is `InvalidTransform`).
`cargo test -p frust-engine --test compile_rects_paths refused` runs the 5 refusal tests; read
`a_refused_frame_records_nothing_at_all` — a bad last command records nothing, and the compiler
compiles the next frame cleanly.

### 11.4 — The GPU layout and the E17 scan, still GPU-free

```bash
cargo test -p frust-engine --test gpu_layouts
cargo test -p frust-engine --test structure
```

Expected: `23 passed`, then `6 passed`. In `gpu_layouts.rs` read
`gpu_strip_is_24_bytes_with_the_shader_field_order`,
`a_strips_alpha_index_becomes_the_column_the_shader_addresses_coverage_by` (`alpha_idx` 3104 →
column 776) and `a_rect_instance_carries_the_rect_flag_in_bit_31`. In `structure.rs`, read
`e17_engine_render_paths_never_unwrap_expect_or_panic_outside_tests` and `production_lines`: the
scan stops at each file's `#[cfg(test)]` or `mod tests` marker and skips comment lines, so tests may
`unwrap()` freely and prose may name the needles. `--test structure e17` runs just those 2 tests.

## What to notice

- The compiler never sees a GPU: strips, alphas, draws, depths, paint records and image uploads
  are plain data on `CompiledFrame`, which is why this whole lab is `cargo test`, not a window.
- Sparse is the point: the solid interior of a fill costs zero alpha bytes; only edges pay. The
  fast path saves CPU (no flatten, no tiling); pixel-aligned edges are what save *alpha*.
- Refusal happens once, up front, over the whole scene. There is no half-recorded frame and no
  second renderer to fall back to (lab 4): a refused frame is simply not drawn.
