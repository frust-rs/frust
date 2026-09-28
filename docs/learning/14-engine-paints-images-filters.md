# Lab 14 — Paints, images, filters and shader quads

**Concept:** A strip only says *where* a draw lands; its colour comes from a paint. A solid colour
rides in the strip instance itself. Everything else — gradients, images, blurred shadows, external
textures — is an **indexed paint**: a record packed into the encoded-paints texture, backed by a
resource with its own residency rules (a gradient ramp cache, an image atlas, a host-registered
texture, an offscreen shader target). This lab reads each resource and runs its GPU-free tests.

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `LutRequest`, `BrushEncoding`, `ImageEncoding` | `crates/frust-engine/src/compile/paint.rs` | ≈51, ≈58, ≈73 |
| `encode_brush` / `encode_image_command` / `encode_image` / `resolve_lut_request` | same file | ≈97–136, ≈181, ≈223–261, ≈312 |
| `GpuEncodedImage` … `GpuEncodedPaint`, `lower_encoded_paint` | `crates/frust-engine/src/gpu/paint_texture.rs` | ≈25–150, ≈282 |
| `PaintType`, `pack_paint_descriptor` (bits of `paint_and_rect_flag`) | `crates/frust-engine/src/gpu/strips.rs` | ≈50, ≈76 |
| `pack_paint` / `resolve_paints` — solid inline vs. indexed record | `crates/frust-engine/src/renderer.rs` | ≈3223, ≈3453 |
| `GradientTextureLayout`, `CachedRamp`, `GradientCache` | `crates/frust-engine/src/cache/gradients.rs` | ≈39, ≈78, ≈106–366 |
| `ImageResidency`, `ImageSkip`, `AtlasBudget`, `is_mobile_tier` | `crates/frust-engine/src/cache/images.rs` | ≈748, ≈231, ≈316, ≈513 |
| `MAX_UNSEEN_FRAMES`, `ATLAS_PADDING`, `MAX_ATLAS_LAYERS` | same file | ≈199, ≈209, ≈222 |
| `allocate_under_pressure`, `plan_pressure_eviction`, `layer_admits` | same file | ≈1237, ≈1502, ≈1581 |
| `atlas_texture_descriptor`, `AtlasArray`, `ensure_layers`, `lower_encoded_image` | `crates/frust-engine/src/gpu/atlas.rs` | ≈162, ≈231, ≈331, ≈548 |
| `inflated_bounds`, `encode_blurred_rounded_rect` | `crates/frust-engine/src/compile/blur_rrect.rs` | ≈100, ≈123 |
| `LayerFilter`, `served_filter`, `SERVED_EDGE_MODE`, `MAX_BLUR_SIGMA` | `crates/frust-engine/src/filters/mod.rs` | ≈103, ≈285, ≈96, ≈85 |
| `blur_passes` / `drop_shadow_passes` | `crates/frust-engine/src/filters/blur.rs` / `crates/frust-engine/src/filters/drop_shadow.rs` | ≈403 / ≈183 |
| `ExternalExtents`, `ExternalSkip`, `encode_scene_texture` | `crates/frust-engine/src/compile/external.rs` | ≈50, ≈124, ≈169 |
| `ExternalTextures`, `lower_encoded_external` | `crates/frust-engine/src/gpu/bindings.rs` | ≈51, ≈200 |
| `ShaderQuadPass`, `frame_demands`, `culled_program_ids` | `crates/frust-engine/src/effects/shader_quad.rs` | ≈455, ≈296, ≈387 |
| `ShaderEffects`, `quantized_target_key`, `MAX_TARGETS_PER_ID` | `crates/frust-gpu/src/effects.rs` | ≈313, ≈207, ≈185 |
| `atlas_disabled`, `atlas_size`, `shader_effects_disabled` | `crates/frust-engine/src/config.rs` | ≈74, ≈154, ≈93 |

## The five ideas

1. **Solid is inline; everything else is a record.** `encode_brush` turns `Brush::Solid` into
   `Paint::Solid` and pushes nothing. `Brush::Gradient` goes through `vello_common`'s `encode_into`
   and comes back `Paint::Indexed` plus a `LutRequest` naming the side-table entry — *unless* the
   gradient is degenerate (under two stops, zero-length line, …), when `vello_common` substitutes a
   solid and no request is made. `Brush::Image` in `encode_brush` is a once-logged transparent
   placeholder; real images go through `encode_image`, which **always** returns `Paint::Indexed`.
   The renderer's `pack_paint` then writes a solid's premultiplied RGBA8 into the strip's `payload`
   with `PaintType::Solid`; an indexed paint gets `PaintPayload::Position` and
   `pack_paint_descriptor(paint_type, texel_offset)` — paint type at bit 26, record texel index in
   bits 0–25 of `paint_and_rect_flag`. The records themselves are `GpuEncodedPaint` variants
   (`lower_encoded_paint` for gradients and blurred rects, `lower_encoded_image` for atlas images,
   `lower_encoded_external` for external textures), each 16-byte aligned with a compile-time size
   assert, serialized back to back into one `Rgba32Uint` texture. How the strip shader decodes
   the flag is lab 13's story.
2. **A gradient ramp is keyed by colour, not geometry.** `GradientCache` stores ramps under
   `vello_common`'s `GradientCacheKey` — stops, interpolation colour space, hue direction. Start/end
   points, radii and the paint transform are absent from the key, so the same gradient under two
   transforms is two encoded entries but one `CachedRamp { lut_start, width }`. Every ramp lives in
   one packed `Rgba8Unorm` byte buffer uploaded as a flat texel stream (`GradientTextureLayout`
   gives the texture's width/height; a ramp may straddle a row). LRU is by an epoch that advances on
   **every lookup**, not per frame, so no two live entries tie. `maintain` (call once per frame,
   after encoding) evicts `len - capacity` entries: `evict` finds the threshold with
   `select_nth_unstable(count - 1)` over last-used epochs, then `compact_luts` closes the holes and
   rewrites survivors' `lut_start` with a prefix sum. `begin_upload` refuses (logs, keeps the old
   texels) rather than truncate when the packed bytes outgrow the texture.
3. **Images are atlas-resident, bounded by age and by pressure.** `ImageResidency` keys an atlas
   rectangle on the `peniko::Blob` id behind a `Command::Image`'s `ImageData`, allocating through
   `vello_common`'s `ImageCache` over a `MultiAtlasManager` — pure rectangle packing, no device, so
   all of it is host-testable. The same `ImageCache` is the glyph atlas's allocator too (one id
   space, one texture array). `AtlasBudget::for_caps` picks `MOBILE` (1024², 4 layers) or `DESKTOP`
   (2048², 8 layers) via `is_mobile_tier`, which reads **two** `TierCaps` fields:
   `downlevel_profile != DownlevelProfile::Full` **or** `transient_saves_memory`. (So on a
   full-profile Vulkan/Metal adapter the choice does come down to `transient_saves_memory` alone;
   a GLES/WebGL2 adapter takes mobile whatever that flag says.) It then clamps to the adapter's
   `max_texture_dimension_2d`/`max_texture_array_layers` and to `MAX_ATLAS_LAYERS` = 256 (the
   record's `atlas_index` has eight bits). A source larger than one layer is shrunk by
   `fit_extent`/`minify` (box filter) rather than refused. An entry unseen for more than
   `MAX_UNSEEN_FRAMES` = 60 frames is reaped at `begin_frame`. When the packer refuses,
   `allocate_under_pressure` first returns `None` at once if every entry was drawn this frame; else
   it models the layers, runs `plan_pressure_eviction` (least-recently-seen candidates, per-layer
   caps of 64 candidates and 4× the requested area, `layer_admits` as the fit test) and only
   releases anything if a layer fits. A request no bounded eviction can place stays a plain
   `ImageSkip::NoAtlasSpace` with nothing evicted. Every `ImageSkip` is a skipped draw, never a
   failed frame; the compiler's `note_image_skip` warns once, then logs at debug.
4. **The atlas texture has GPU quirks the packer doesn't.** `ATLAS_PADDING` = 0 because both the
   bilinear and bicubic samplers in the strip shader clamp every tap into the image's own
   rectangle. `atlas_texture_descriptor` raises the layer count to **two**, never one — its doc
   comment: *"wgpu-hal 30.0.1's GLES backend picks a texture's GL target from the descriptor alone
   … A one-layer array descriptor therefore binds as plain `GL_TEXTURE_2D` while … the strip
   shader always samples this texture as a `sampler2DArray` … and every sample returns
   `(0, 0, 0, 1)`"* (registered as `engine-webgl2-atlas-target` in `docs/LIMITATIONS.md`). Growth
   is `AtlasArray::ensure_layers`: a deeper texture plus a layer copy recorded in **its own**
   encoder and submitted before the frame's writes — lab 4's first sanctioned own-encoder exception,
   because a copy in the frame's encoder would run after that frame's `write_texture` uploads and
   restore stale contents over them.
5. **Shadows, filters and external pixels are paints too — with different owners.**
   - *Blurred rounded rect:* the strip coverage is `inflated_bounds(rect, σ)` = `rect` inflated by
     `min(2.5σ, MAX_KERNEL_PAD)`; the shape is evaluated per pixel from a `GpuBlurredRoundedRect`
     record. `encode_blurred_rounded_rect` collapses `CornerRadii` with `largest()` because
     `vello_common`'s type carries one radius (`render-blurred-shadow-corner-collapse` in
     `docs/LIMITATIONS.md`).
   - *Layer filters* are engine-internal: `LayerFilter::{Blur { sigma }, DropShadow { offset,
     sigma, color }}`, opened with `push_filter_layer` — `frust_scene` has no filter command.
     `served_filter` refuses everything else by name (its message is the `SERVED_FILTERS` string),
     including any edge mode but `SERVED_EDGE_MODE` = `EdgeMode::None`; `MAX_BLUR_SIGMA` = 4096.0
     stops the 3σ layer growth from wrapping its `u16`. `blur_passes` plans
     downscale ×n → `BlurH` → `BlurV` → upscale ×n (always even, so the result lands back in the
     layer's own page); `drop_shadow_passes` wraps that in `Offset` … `Colorize`. Each pass is a
     *filter round* — one instanced quad through `crates/frust-engine/shaders/filter.wgsl` (with
     `crates/frust-engine/shaders/filters_blur.wgsl` and
     `crates/frust-engine/shaders/filters_drop_shadow.wgsl` prepended) that draws no strips.
   - *External textures:* `Command::SceneTexture` and `Command::ShaderQuad` both end in
     `draw_external_texture` (`crates/frust-engine/src/compile/mod.rs` ≈1343). `ExternalExtents`
     mirrors the texel sizes the host bound via `EngineRenderer::bind_texture`; `bind` refuses a
     zero or >`u16::MAX` side. `encode_scene_texture` is always `Paint::Indexed`, always
     `may_have_transparency: true` (so it blends, never occludes); an unbound id is
     `ExternalSkip::Unregistered`, warned once per id, then debug. The renderer lowers it with
     `lower_encoded_external` into the *image* record with the source-kind bit set, resolving the
     view through `ExternalTextures`.
   - *Shader quads:* `ShaderQuadPass::prepare` runs before the frame, in the same encoder.
     `frame_demands` collapses every quad of one program into one demand (largest extent per axis,
     first quad's time) and, when an extent was supplied with `set_frame_target_extent`, drops
     quads entirely off-target (`quad_is_culled`, skipped inside a snapshot bracket). frust-render's
     `SurfaceRenderer` builds the pass but never calls that setter today (open minor
     act_000001a064935774), so on the live path off-screen quads still get targets. The GPU half is
     `frust_gpu::effects::ShaderEffects`: lazy per-program pipeline seeded from the persisted
     pipeline cache, a fullscreen-triangle pass viewport-confined to the exact requested size,
     targets keyed by `quantized_target_key` (rounded up to the 256 px pool quantum),
     `MAX_TARGETS_PER_ID` = 2, and age-based reaps (120 frames per program, 60 per target key).
     `FRUST_ENGINE_NO_SHADER_EFFECTS` turns the whole thing off.

Knobs: `FRUST_ENGINE_NO_ATLAS` (every resolution answers `ImageSkip::AtlasDisabled`, so every
image draw is a logged skip); `FRUST_ENGINE_ATLAS_SIZE=<W>x<H>` (redistributes the tier's bytes
between layer extent and depth via `within_total_bytes` — never more than the tier would spend).

## Experiments

From the repo root. Never run `cargo test -p frust-engine` bare or `--test proptest_strips`.

### 14.1 — Solid inline, gradient indexed

```bash
cargo test -p frust-engine --test paint_encoding
```

15 tests. `solid_brush_encodes_to_a_premultiplied_paint_with_no_side_table_entry` asserts
`paints.is_empty()` and `lut_request == None`; `each_gradient_kind_encodes_to_its_encoded_kind`
puts linear/radial/sweep in the side table; `degenerate_gradient_falls_back_to_a_solid_with_no_lut_request`
is the exception. The last four tests check record bytes and texel offsets without a device.

### 14.2 — One ramp, two placements

```bash
cargo test -p frust-engine --lib cache::gradients   # 2 tests: upload refusal vs. exact fit
```

Then write a throwaway integration test (delete it afterwards) that encodes one two-stop linear
gradient under `Affine::IDENTITY` and under `Affine::rotate(0.7).then_translate((40.0, 90.0).into())`,
resolves both `LutRequest`s against a `GradientCache::new(8, Level::new())`, and asserts:

```rust
assert_eq!(paints.len(), 2); assert_ne!(ta, tb); // two entries, different inverse transforms
assert_eq!(cache.entry_count(), 1); assert_eq!(ra, rb); // one ramp: {lut_start: 0, width: 256}
```

`gradients_differing_only_in_geometry_share_one_ramp` (`paint_encoding`) is the committed twin.

### 14.3 — Reap, and a refusal that costs nothing

```bash
cargo test -p frust-engine --test images
```

32 run (34 declared; the `perf-trace`-gated residency-line tests compile out by default).
`an_unseen_image_is_reaped_and_its_rectangle_reported_for_clearing` compiles 60 empty frames with
no eviction, then sees the rectangle come back on the 61st (the reap tests `> MAX_UNSEEN_FRAMES`).
`a_request_no_eviction_could_satisfy_costs_no_eviction_at_all` draws a 48² image among four 32²
images in a four-slot atlas for eight frames: one skip per frame, zero uploads and clears after
frame 0, `pressure_evictions() == 0`. Also try `--test atlas_churn` (47 pass, 2 GPU-ignored).

### 14.4 — What filters are served

```bash
cargo test -p frust-engine --test filters
```

55 pass, 7 ignored (GPU). Refusals: `every_filter_but_the_blur_or_drop_shadow_is_still_refused_by_name`
(a flood; the reason must say "Gaussian blur"), `a_drop_shadow_that_composites_the_original_is_refused_by_name`,
`an_edge_mode_the_kernels_do_not_implement_is_refused_by_name`. The pass plan is
`a_blur_is_its_decimations_a_convolution_and_the_way_back`. `--test blur_rrect` (14) is the shadow.

### 14.5 — Culling and quantisation

```bash
cargo test -p frust-engine --lib effects::shader_quad   # 42: clamp_size, requested_size, culling
cargo test -p frust-gpu --lib quantized_target_key       # 5: the 256 px quantum lives here
```

Read `a_quad_entirely_outside_the_target_extent_demands_nothing` next to
`with_no_target_extent_a_far_off_screen_quad_still_demands` — the second is the live path today.
`crates/frust-engine/tests/shader_quad.rs` is all GPU-ignored.

### 14.6 — Follow one `Command::Image` to a texel (read-only)

`crates/frust-engine/src/compile/mod.rs` `Command::Image` arm (≈1184) → the paint step's
`PaintSource::Image` → `encode_image_command` (`natural_to_dest` maps the natural pixel rect onto
`dest`) → `encode_image` (inverts the transform, `ImageResidency::resolve`, pushes
`EncodedPaint::Image`, returns `Paint::Indexed`) → renderer `resolve_paints` → `lower_encoded_image`
packs layer, offset, size, quality and extends into a `GpuEncodedImage` → `pack_paint` writes
`PaintType::Image` + texel offset into `paint_and_rect_flag` → in
`crates/frust-engine/shaders/strip.wgsl` the `PAINT_TYPE_IMAGE` branch reads three texels with
`load_encoded_paint_texel`, applies the extend modes, and samples `atlas_texture_array` at the
record's layer (`textureLoad` for Low, `bilinear_sample` for Medium, `bicubic_sample` for High) —
or the external binding when the source-kind bit is set.

## What to notice before moving on

- Every resource here refuses by **skipping a draw**, never the frame: `ImageSkip`, `ExternalSkip`,
  a dropped shader quad, a gradient with no ramp. The exception: a filter `served_filter` refuses
  skips the whole frame — there is no second renderer to fall back to.
- Residency is **committed only once serviced**: `ImageResidency::plan` keeps re-emitting uploads
  until `acknowledge_plan`, so a frame refused after compiling does not leave an image recorded as
  resident with unwritten texels.
- The atlas tier choice is a driver answer, not a memory class — see
  `engine-atlas-tier-by-driver-flag` in `docs/LIMITATIONS.md`, and compare its wording with
  `is_mobile_tier` yourself.
