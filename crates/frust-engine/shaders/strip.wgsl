// Copyright 2024 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Derived from vello_sparse_shaders 0.2.0 (`shaders/render.wesl`); its
// `import package::helpers::...` lines are resolved by prepending
// `helpers.wgsl` at load time (`gpu::shader_src`).
//
// Renders sparse strips with alpha blending. Each strip instance is a
// horizontal slice of the output made of:
//
// 1. a variable-width region of alpha values for semi-transparent rendering, and
// 2. a solid region for fully opaque areas.
//
// The alpha values live in a texture and are sampled during fragment shading,
// so coverage is stored only where it is actually needed.
//
// `StripInstance::paint_and_rect_flag` encodes a color source, a paint type
// and a paint texture id. The color source says where the fragment shader
// reads color data from, the paint type says how it uses that data, and the
// paint texture id locates the encoded paint record in
// `encoded_paints_texture`. `StripInstance::payload` then carries either a
// packed color, an [x, y] sample coordinate, a layer texture origin, or a
// glyph quad's atlas texel. See
// the `StripInstance` comment below for the full bit layout.
//
// This module has six render-state variants over the same two entry points
// (`gpu::pipelines`): an intermediate-target variant, an alpha-blended
// variant with and without a depth attachment, an opaque variant that writes
// depth, and a destination-out pair for the hole punch.
//
// Paint-class specialisation. The fragment stage's colour-source dispatch is
// what a fragment pays for whether or not it takes a branch, so the engine
// compiles this ONE source into several programs, each keeping only the
// branches one paint class needs (`gpu::shader_src::strip_class_source`).
// A block between a `//#if FEATURE` line and its `//#endif` line is kept only
// by the classes that enable FEATURE; everything outside such a block —
// coverage, the solid paint, the vertex stage, every binding — is in every
// class. The features are:
//
//   IMAGE          — atlas image sampling, nearest and bilinear
//   IMAGE_FILTERED — inside IMAGE: bicubic atlas sampling and the external
//                    texture (a caller-owned texture, any quality)
//   GRADIENT       — linear, radial and sweep gradients
//   BLURRED_RECT   — the blurred rounded rectangle
//   LAYER          — the layer colour source a composite reads its page through
//   GLYPH          — the glyph quad: one nearest atlas load at the texel the
//                    payload names, no paint transform and no extend mode
//
// One more feature is not a class but a program FAMILY, chosen per renderer
// (`FRUST_ENGINE_IMAGE_SAMPLER`, on unless set to 0):
//
//   IMAGE_SAMPLER  — inside IMAGE: bilinear atlas sampling through a hardware
//                    sampler instead of four loads; its `//#else` arm is the
//                    manual path every program of the manual family keeps
//
// The directive lines are plain comments, and every class feature's block is
// a whole arm of its own, so the class that enables every class feature — the
// Uber program — is this file with one of the two IMAGE_SAMPLER arms blanked
// out: the `//#else` arm in the sampler family, the sampler arm in the manual
// one.
// Every class of a family binds the same groups through one explicit pipeline
// layout, so a binding a class no longer reads is still declared here.

// Color source modes: where the fragment shader gets color data from.
// Use the payload (a color or image coordinates).
const COLOR_SOURCE_PAYLOAD: u32 = 0u;
// Sample from a rendered layer texture.
const COLOR_SOURCE_LAYER: u32 = 1u;

// Paint types.
const PAINT_TYPE_SOLID: u32 = 0u;
const PAINT_TYPE_IMAGE: u32 = 1u;
const PAINT_TYPE_LINEAR_GRADIENT: u32 = 2u;
const PAINT_TYPE_RADIAL_GRADIENT: u32 = 3u;
const PAINT_TYPE_SWEEP_GRADIENT: u32 = 4u;
const PAINT_TYPE_BLURRED_ROUNDED_RECT: u32 = 5u;
// A cached glyph drawn as one rect instance; see the decision tree below.
const PAINT_TYPE_GLYPH: u32 = 6u;

// Paint texture index mask (the low 26 bits of the paint field).
const PAINT_TEXTURE_INDEX_MASK: u32 = 0x03FFFFFFu;

const RECT_STRIP_FLAG: u32 = 0x80000000u;

// Must stay byte-identical to `GpuConfig` in `gpu::config`.
struct Config {
    // Width of the rendering target.
    width: u32,
    // Height of the rendering target.
    height: u32,
    // Height of a strip in pixels.
    // CAUTION: changing this value also requires changing the fragment
    // shader's alpha unpacking, which assumes one channel per strip column.
    strip_height: u32,
    // Number of trailing zeros in the alpha texture's width (its log2),
    // pre-computed on the CPU because a downlevel (GLES 3.0 / WebGL2) target
    // has no bit-scan intrinsic to derive it here.
    alphas_tex_width_bits: u32,
    // Number of trailing zeros in the encoded-paint texture's width, for the
    // same reason as `alphas_tex_width_bits`.
    encoded_paints_tex_width_bits: u32,
    // An offset applied to every strip.
    //
    // Usually zero. Rendering a filter layer needs it to account for both the
    // shift caused by rendering only the tight bounding box of that layer and
    // the offset of the layer's destination within an atlas.
    strip_offset_x: i32,
    strip_offset_y: i32,
    // Whether to flip the y component of the NDC coordinates.
    negate_ndc: u32,
}

// A `StripInstance` is either a **normal strip** (a sparse fill or alpha fill
// of height `Config::strip_height`) or a **rect strip** (a whole rectangle
// drawn as one quad, with anti-aliasing). The two are distinguished by
// RECT_STRIP_FLAG (bit 31 of `paint_and_rect_flag`).
//
// The fields are read differently in each mode:
//
//   Field                 | Normal strip                      | Rect strip
//   ----------------------+-----------------------------------+-----------------------------------
//   xy                    | Strip position                    | Rect top-left (snapped outward)
//   widths_or_rect_height | [width, dense_width]              | [width, height] (both snapped)
//   col_idx_or_rect_frac  | Alpha column index                | Packed AA edge fractions (4 x u8)
//   payload               | Color / scene coords / layer xy   | Color / scene coords / layer xy
//                         |                                   | / glyph atlas xy
//   paint_and_rect_flag   | Paint encoding                    | Paint encoding | RECT_STRIP_FLAG
//
// `paint_and_rect_flag` bit layout:
//   - Bit  31:    `RECT_STRIP_FLAG`  0 = normal strip, 1 = rect strip
//   - Bits 29-30: `color_source`     0 = use payload, 1 = use layer texture
//   - Bits 0-28:  Usage depends on color_source:
//
//     When color_source = 0 (COLOR_SOURCE_PAYLOAD):
//       - Bits 26-28: `paint_type` (0 = solid, 1 = image, 2 = linear gradient,
//         3 = radial gradient, 4 = sweep gradient, 5 = blurred rounded rect,
//         6 = glyph quad)
//       - Bits 0-25:
//         - If paint_type = 0: unused
//         - If paint_type >= 1: `paint_texture_idx`
//
//     When color_source = 1 (COLOR_SOURCE_LAYER):
//       - Bits 0-7: opacity (0-255)
//       - Bits 8-28: unused
//
// Decision tree for paint/payload interpretation:
//
// color_source = 0 (COLOR_SOURCE_PAYLOAD) - use the payload directly
// |-- paint_type = 0 (PAINT_TYPE_SOLID)
// |   \-- payload = [r, g, b, a] RGBA (packed as u8s)
// |
// |-- paint_type = 1 (PAINT_TYPE_IMAGE)
// |   \-- payload = packed image parameters
// |
// |-- paint_type = 2/3/4 (LINEAR / RADIAL / SWEEP gradient)
// |   |-- payload = [x, y] scene coordinates (packed as u16s)
// |   \-- bits 0-25 = paint_texture_idx
// |-- paint_type = 5 (PAINT_TYPE_BLURRED_ROUNDED_RECT)
// |   |-- payload = [x, y] scene coordinates (packed as u16s)
// |   \-- bits 0-25 = paint_texture_idx
// \-- paint_type = 6 (PAINT_TYPE_GLYPH), rect strips only
//     |-- payload = [x, y] atlas texel under the rect's top-left corner
//     |   (packed as u16s)
//     \-- bits 0-25 = paint_texture_idx of the glyph run's record, read for
//         its atlas layer and tint alone
//
// color_source = 1 (COLOR_SOURCE_LAYER) - use the rendered layer texture
// |-- payload = [x, y] source layer texture origin (packed as u16s)
// \-- bits 0-7 = opacity
//
// Must stay byte-identical to `GpuStrip` in `gpu::strips`.
struct StripInstance {
    // [x, y] packed as u16s: the coordinates of the strip or rect.
    @location(0)
    xy: u32,
    // [width, dense_width] packed as u16s.
    // width — width of the strip or rect.
    // dense_width — width of the portion alpha blending applies to. For an
    // anti-aliased strip width = dense_width; for a sparse fill region
    // dense_width = 0. For a rect strip, dense_width holds the rect height
    // instead.
    @location(1)
    widths_or_rect_height: u32,
    // For normal strips: the alpha texture column index this strip's alpha
    // values begin at. There are `Config::strip_height` alpha values per
    // column. For rect strips: packed fractional edge offsets for AA.
    @location(2)
    col_idx_or_rect_frac: u32,
    // See the StripInstance comment above.
    @location(3)
    payload: u32,
    // See the StripInstance comment above.
    @location(4)
    paint_and_rect_flag: u32,
    // Painter's-order index driving the z-depth computation.
    @location(5)
    depth_index: u32,
}

struct VertexOutput {
    // Paint encoding plus the rect flag for this strip.
    @location(0) @interpolate(flat)
    paint_and_rect_flag: u32,
    // Texture coordinates for the current fragment.
    @location(1)
    tex_coord: vec2<f32>,
    // Coordinates the paint is sampled at, used for images and gradients.
    @location(2)
    sample_xy: vec2<f32>,
    // For normal strips: the ending x position of the dense (alpha) region.
    // For rect strips: packed dimensions (width | height << 16).
    @location(3) @interpolate(flat)
    dense_end_or_rect_size: u32,
    // Packed paint payload or layer sample coordinate.
    @location(4) @interpolate(flat)
    payload: u32,
    // Packed fractional edge offsets for rectangles.
    // Bits 0-7: x0, 8-15: y0, 16-23: x1, 24-31: y1. Zero for normal strips.
    @location(5) @interpolate(flat)
    rect_frac: u32,
    // Normalized device coordinates (NDC) for the current vertex.
    @builtin(position)
    position: vec4<f32>,
};

@group(0) @binding(0)
var alphas_texture: texture_2d<u32>;

@group(0) @binding(1)
var<uniform> config: Config;

@group(0) @binding(2)
var layer_input_texture: texture_2d<f32>;

@group(1) @binding(0)
var atlas_texture_array: texture_2d_array<f32>;

@group(1) @binding(1)
var external_texture: texture_2d<f32>;

// Declared only by the image programs of the sampler family, together with
// the layout entry it binds against (`gpu::bindings`) and the one function
// that samples through it; every other program — and every program of the
// manual family — declares no sampler at all.
//#if IMAGE_SAMPLER
@group(1) @binding(2)
var atlas_sampler: sampler;

// Bilinear filtering through a hardware sampler: what `bilinear_sample`
// computes, in one filtered fetch instead of four loads.
//
// `coords` is the texel-space sample position with texel i's centre at
// i + 0.5 — `bilinear_sample`'s own `coords` plus half a texel. It is clamped
// to the centres of the image's edge texels, which is exactly the clamp the
// manual path applies to its taps: a tap never leaves the image's own atlas
// rectangle, so a neighbouring slot's texels never reach the filter, for any
// source rectangle and any extend mode, with no padding around the slot. Only
// the precision of the weights differs — a sampler interpolates at the
// hardware's sub-texel resolution rather than in full `f32`. Lives here
// rather than in the helper prelude because no other module may sample: the
// filter program's one bilinear sample is its own region-bounded helper.
fn sampled_bilinear(
    tex: texture_2d_array<f32>,
    linear_sampler: sampler,
    coords: vec2<f32>,
    atlas_idx: i32,
    image_offset: vec2<f32>,
    image_size: vec2<f32>,
) -> vec4<f32> {
    let centre = clamp(coords, image_offset + vec2(0.5), image_offset + image_size - vec2(0.5));
    let uv = centre / vec2<f32>(textureDimensions(tex));
    return textureSampleLevel(tex, linear_sampler, uv, atlas_idx, 0.0);
}
//#endif

@group(2) @binding(0)
var encoded_paints_texture: texture_2d<u32>;

@group(3) @binding(0)
var gradient_texture: texture_2d<f32>;

// Convert a flat texel index to 2D coordinates in the encoded-paints texture.
fn encoded_paint_coord(flat_idx: u32) -> vec2<u32> {
    return vec2<u32>(
        flat_idx & ((1u << config.encoded_paints_tex_width_bits) - 1u),
        flat_idx >> config.encoded_paints_tex_width_bits
    );
}

fn load_encoded_paint_texel(paint_tex_idx: u32, texel_offset: u32) -> vec4<u32> {
    return textureLoad(
        encoded_paints_texture,
        encoded_paint_coord(paint_tex_idx + texel_offset),
        0,
    );
}

@vertex
fn vs_main(
    @builtin(vertex_index) in_vertex_index: u32,
    instance: StripInstance,
) -> VertexOutput {
    var out: VertexOutput;
    out.sample_xy = vec2(0.0);
    // Map vertex_index (0-3) to quad corners:
    // 0 -> (0,0), 1 -> (1,0), 2 -> (0,1), 3 -> (1,1)
    let corner = quad_corner(in_vertex_index);
    let x = corner.x;
    let y = corner.y;
    // Unpack the x and y coordinates from the packed u32 instance.xy.
    let strip_position = unpack_u16_pair(instance.xy);
    let widths = unpack_u16_pair(instance.widths_or_rect_height);
    let x0 = strip_position.x;
    let y0 = strip_position.y;
    let width = widths.x;
    let dense_width = widths.y;

    let is_rect = (instance.paint_and_rect_flag & RECT_STRIP_FLAG) != 0u;
    var height = config.strip_height;
    if is_rect {
        height = dense_width;
        out.dense_end_or_rect_size = width | (dense_width << 16u);
        out.rect_frac = instance.col_idx_or_rect_frac;
    } else {
        out.dense_end_or_rect_size = instance.col_idx_or_rect_frac + dense_width;
        out.rect_frac = 0u;
    }
    // Pixel coordinates of this vertex within the strip, with the strip
    // offset applied.
    let pixel = vec2<f32>(
        f32(i32(x0) + config.strip_offset_x) + x * f32(width),
        f32(i32(y0) + config.strip_offset_y) + y * f32(height),
    );
    // Convert pixel coordinates to normalized device coordinates, which range
    // from -1 to 1 with (0,0) at the center of the viewport.
    let ndc = pixel_to_ndc(pixel, vec2<f32>(f32(config.width), f32(config.height)));

    let color_source = (instance.paint_and_rect_flag >> 29u) & 0x3u;
    if color_source == COLOR_SOURCE_PAYLOAD {
        let paint_type = (instance.paint_and_rect_flag >> 26u) & 0x7u;
        // Unpack the view coordinates used for image sampling and gradients.
        let scene_strip = unpack_u16_pair(instance.payload);

        if paint_type == PAINT_TYPE_IMAGE {
            let paint_tex_idx = instance.paint_and_rect_flag & PAINT_TEXTURE_INDEX_MASK;
            let image_texel0 = load_encoded_paint_texel(paint_tex_idx, 0u);
            let image_texel1 = load_encoded_paint_texel(paint_tex_idx, 1u);
            let image_texel2 = load_encoded_paint_texel(paint_tex_idx, 2u);
            // Image sampling always happens in global view space.
            let pos = vec2<f32>(
                f32(scene_strip.x) + x * f32(width),
                f32(scene_strip.y) + y * f32(height),
            );
            out.sample_xy = get_image_translate(image_texel1, image_texel2)
                + get_image_offset(image_texel0)
                + get_image_transform(image_texel0, image_texel1) * pos;
        } else if paint_type == PAINT_TYPE_LINEAR_GRADIENT || paint_type == PAINT_TYPE_RADIAL_GRADIENT || paint_type == PAINT_TYPE_SWEEP_GRADIENT || paint_type == PAINT_TYPE_BLURRED_ROUNDED_RECT || paint_type == PAINT_TYPE_GLYPH {
            // The gradient transform is likewise applied in global view space.
            // A glyph quad's payload is already an atlas position, so the same
            // interpolation walks the glyph's slot texel for texel.
            out.sample_xy = vec2<f32>(
                f32(scene_strip.x) + x * f32(width),
                f32(scene_strip.y) + y * f32(height)
            );
        }
    } else if color_source == COLOR_SOURCE_LAYER {
        let source = unpack_u16_pair(instance.payload);
        out.sample_xy = vec2<f32>(
            f32(source.x) + x * f32(width),
            f32(source.y) + y * f32(height),
        );
    }

    let col_offset = select(f32(instance.col_idx_or_rect_frac), 0.0, is_rect);
    out.tex_coord = vec2<f32>(col_offset + x * f32(width), y * f32(height));

    // Divide by a power of two so the arithmetic is exact in f32, and by the
    // expected 24 bits of depth-buffer precision.
    let z = 1.0 - f32(instance.depth_index) / f32(1u << 24u);
    // Flip y based on the config flag.
    let final_ndc_y = select(ndc.y, -ndc.y, config.negate_ndc != 0u);
    out.position = vec4<f32>(ndc.x, final_ndc_y, z, 1.0);
    out.payload = instance.payload;
    out.paint_and_rect_flag = instance.paint_and_rect_flag;

    return out;
}

@fragment
fn fs_main(
    @location(0) @interpolate(flat) paint_and_rect_flag: u32,
    @location(1) tex_coord: vec2<f32>,
    @location(2) sample_xy: vec2<f32>,
    @location(3) @interpolate(flat) dense_end_or_rect_size: u32,
    @location(4) @interpolate(flat) payload: u32,
    @location(5) @interpolate(flat) rect_frac: u32,
    @builtin(position) position: vec4<f32>,
) -> @location(0) vec4<f32> {
    var alpha = 1.0;
    let is_rect = (paint_and_rect_flag & RECT_STRIP_FLAG) != 0u;
    if is_rect && rect_frac != 0u {
        let frac = unpack4x8unorm(rect_frac);
        // How much of the pixel the rect actually covers: the fractions in
        // the x and y direction, multiplied. Both directions are computed in
        // one pass by packing them into a vec2.
        let rect_size = vec2<f32>(unpack_u16_pair(dense_end_or_rect_size));
        let tc = tex_coord;
        // +0.5 / -0.5 because the fragment shader positions coordinates at
        // the center of the pixel.
        let bottom_and_right = min(tc + 0.5, rect_size - frac.zw);
        let top_and_left = max(tc - 0.5, frac.xy);
        let a = clamp(bottom_and_right - top_and_left, vec2(0.0), vec2(1.0));
        alpha = a.x * a.y;
    } else if !is_rect && dense_end_or_rect_size != 0u {
        let x = u32(floor(tex_coord.x));
        let y = u32(floor(tex_coord.y));
        // Retrieve the alpha value from the texture. 16 1-byte alpha values
        // live in one texel, with each color channel packing 4 of them. This
        // assumes a strip height of 4, i.e. each channel encodes the alpha
        // values of a single column within a strip.
        let alphas_index = x;
        // Which texel holds the alpha values for this column.
        let texel_index = alphas_index / 4u;
        // Which channel (R,G,B,A) of that texel holds them.
        let channel_index = alphas_index % 4u;
        // The texture's width is `1 << alphas_tex_width_bits` by construction
        // (`gpu::config`), so the column within the row is a mask of the same
        // shift the row is — no `textureDimensions` query per fragment.
        let tex_x = texel_index & ((1u << config.alphas_tex_width_bits) - 1u);
        let tex_y = texel_index >> config.alphas_tex_width_bits;

        // Load all 4 channels from the texture.
        let rgba_values = textureLoad(alphas_texture, vec2<u32>(tex_x, tex_y), 0);

        // Take the column's alphas from the channel the index selects.
        let alphas_u32 = unpack_alphas_from_channel(rgba_values, channel_index);
        // Extract the alpha for the current y position from the packed u32.
        alpha = f32((alphas_u32 >> (y * 8u)) & 0xffu) * (1.0 / 255.0);
    }
    // Apply the alpha value to the unpacked RGBA color or the sampled paint.
    let color_source = (paint_and_rect_flag >> 29u) & 0x3u;
    var final_color: vec4<f32>;

    if color_source == COLOR_SOURCE_PAYLOAD {
        let paint_type = (paint_and_rect_flag >> 26u) & 0x7u;

        // `payload` encodes a color for PAINT_TYPE_SOLID, or sample
        // coordinates for the other paint types.
        if paint_type == PAINT_TYPE_SOLID {
            final_color = alpha * unpack4x8unorm(payload);
//#if IMAGE
        } else if paint_type == PAINT_TYPE_IMAGE {
            let paint_tex_idx = paint_and_rect_flag & PAINT_TEXTURE_INDEX_MASK;
            let image_texel0 = load_encoded_paint_texel(paint_tex_idx, 0u);
            let image_texel1 = load_encoded_paint_texel(paint_tex_idx, 1u);
            let image_texel2 = load_encoded_paint_texel(paint_tex_idx, 2u);
            let image_offset = get_image_offset(image_texel0);
            let image_size = get_image_size(image_texel0);
            let image_extend_modes = get_image_extend_modes(image_texel0);
            let image_atlas_index = get_image_atlas_index(image_texel0);
            let image_quality = get_image_quality(image_texel0);
            let image_source_kind = get_image_source_kind(image_texel0);
            let image_padding = get_image_padding(image_texel2);
            let packed_tint = image_texel2.y;
            let image_tint = unpack4x8unorm(packed_tint);
            let is_multiply = image_texel2.z == TINT_MODE_MULTIPLY;
            let local_xy = sample_xy - image_offset;
            // A small offset the CPU rasterizer does not need: 45-degree
            // skewing produces artifacts on the GPU without it. Gradients
            // carry an equivalent bias below.
            let offset = 0.00001;
            let extended_xy = vec2<f32>(
                extend_mode(local_xy.x + offset, image_extend_modes.x, image_size.x),
                extend_mode(local_xy.y + offset, image_extend_modes.y, image_size.y)
            );

            var sample_color: vec4<f32>;
//#if IMAGE_FILTERED
            if image_source_kind == IMAGE_SOURCE_EXTERNAL {
                let final_xy = image_offset + extended_xy;
                sample_color = sample_external_image(
                    external_texture,
                    image_quality,
                    final_xy,
                    image_offset,
                    image_size,
                );
            } else if image_quality == IMAGE_QUALITY_HIGH {
                let final_xy = image_offset + extended_xy;
                sample_color = bicubic_sample(
                    atlas_texture_array,
                    final_xy,
                    i32(image_atlas_index),
                    image_offset,
                    image_size,
                    image_extend_modes,
                    image_padding,
                );
            } else
//#endif
            if image_quality == IMAGE_QUALITY_MEDIUM {
//#if IMAGE_SAMPLER
                sample_color = sampled_bilinear(
                    atlas_texture_array,
                    atlas_sampler,
                    image_offset + extended_xy,
                    i32(image_atlas_index),
                    image_offset,
                    image_size,
                );
//#else
                let final_xy = image_offset + extended_xy - vec2(0.5);
                sample_color = bilinear_sample(
                    atlas_texture_array,
                    final_xy,
                    i32(image_atlas_index),
                    image_offset,
                    image_size,
                    image_extend_modes,
                    image_padding,
                );
//#endif
            } else {
                let final_xy = image_offset + extended_xy;
                sample_color = textureLoad(
                    atlas_texture_array,
                    vec2<u32>(final_xy),
                    i32(image_atlas_index),
                    0,
                );
            }

            final_color = alpha * select(
                image_tint * sample_color.a,
                sample_color * image_tint,
                is_multiply
            );
//#endif
//#if GRADIENT
        } else if paint_type == PAINT_TYPE_LINEAR_GRADIENT {
            let paint_tex_idx = paint_and_rect_flag & PAINT_TEXTURE_INDEX_MASK;
            let gradient_texel0 = load_encoded_paint_texel(paint_tex_idx, 0u);
            let gradient_texel1 = load_encoded_paint_texel(paint_tex_idx, 1u);

            // Fragment position with the gradient's affine transform applied.
            let fragment_pos = sample_xy;
            let grad_pos = apply_gradient_transform(gradient_texel0, gradient_texel1, fragment_pos);

            // For a linear gradient the t value is just the x coordinate in
            // gradient space.
            let t_value = grad_pos.x + 0.00001;
            let gradient_color = sample_gradient_lut(
                gradient_texture,
                t_value,
                get_gradient_extend_mode(gradient_texel0),
                get_gradient_start(gradient_texel0),
                get_gradient_texture_width(gradient_texel0),
                config.encoded_paints_tex_width_bits
            );
            final_color = alpha * gradient_color;
        } else if paint_type == PAINT_TYPE_RADIAL_GRADIENT {
            let paint_tex_idx = paint_and_rect_flag & PAINT_TEXTURE_INDEX_MASK;
            let gradient_texel0 = load_encoded_paint_texel(paint_tex_idx, 0u);
            let gradient_texel1 = load_encoded_paint_texel(paint_tex_idx, 1u);
            let gradient_texel2 = load_encoded_paint_texel(paint_tex_idx, 2u);
            let gradient_texel3 = load_encoded_paint_texel(paint_tex_idx, 3u);

            let fragment_pos = sample_xy;
            let grad_pos = apply_gradient_transform(gradient_texel0, gradient_texel1, fragment_pos);

            // For a radial gradient, evaluate the distance from the center.
            let gradient_result = calculate_radial_gradient(grad_pos, gradient_texel2, gradient_texel3);
            let gradient_color = sample_gradient_lut(
                gradient_texture,
                gradient_result.x,
                get_gradient_extend_mode(gradient_texel0),
                get_gradient_start(gradient_texel0),
                get_gradient_texture_width(gradient_texel0),
                config.encoded_paints_tex_width_bits
            );
            final_color = select(
                vec4<f32>(0.0, 0.0, 0.0, 0.0),
                alpha * gradient_color,
                gradient_result.y != 0.0
            );
        } else if paint_type == PAINT_TYPE_SWEEP_GRADIENT {
            let paint_tex_idx = paint_and_rect_flag & PAINT_TEXTURE_INDEX_MASK;
            let gradient_texel0 = load_encoded_paint_texel(paint_tex_idx, 0u);
            let gradient_texel1 = load_encoded_paint_texel(paint_tex_idx, 1u);
            let gradient_texel2 = load_encoded_paint_texel(paint_tex_idx, 2u);

            let fragment_pos = sample_xy;
            var grad_pos = apply_gradient_transform(gradient_texel0, gradient_texel1, fragment_pos);

            // Bias very small coordinates to zero before the angle
            // calculation. The angle calculation picks a quadrant from the
            // coordinates' signs, so for coordinates around zero slight noise
            // lands it in different quadrants from frame to frame; the
            // resulting flicker is very visible because the sweep gradient's
            // seam is not anti-aliased, and it varies across machines.
            grad_pos = select(grad_pos, vec2(0.0), abs(grad_pos) < vec2(NEARLY_ZERO_TOLERANCE));

            // For a sweep gradient, take the angle from the center using the
            // fast polynomial approximation.
            let unit_angle = xy_to_unit_angle(grad_pos.x, grad_pos.y);
            // Convert the unit angle [0, 1) to radians [0, 2*PI).
            let angle = unit_angle * TWO_PI;
            let t_value = (angle - get_sweep_start_angle(gradient_texel2)) * get_sweep_inv_angle_delta(gradient_texel2);
            let gradient_color = sample_gradient_lut(
                gradient_texture,
                t_value,
                get_gradient_extend_mode(gradient_texel0),
                get_gradient_start(gradient_texel0),
                get_gradient_texture_width(gradient_texel0),
                config.encoded_paints_tex_width_bits
            );
            final_color = alpha * gradient_color;
//#endif
//#if BLURRED_RECT
        } else if paint_type == PAINT_TYPE_BLURRED_ROUNDED_RECT {
            let paint_tex_idx = paint_and_rect_flag & PAINT_TEXTURE_INDEX_MASK;
            let blurred_texel0 = load_encoded_paint_texel(paint_tex_idx, 0u);
            let blurred_texel1 = load_encoded_paint_texel(paint_tex_idx, 1u);
            let blurred_texel2 = load_encoded_paint_texel(paint_tex_idx, 2u);
            let blurred_texel3 = load_encoded_paint_texel(paint_tex_idx, 3u);
            let blurred_texel4 = load_encoded_paint_texel(paint_tex_idx, 4u);
            final_color = alpha * calculate_blurred_rounded_rect(
                sample_xy,
                blurred_texel0,
                blurred_texel1,
                blurred_texel2,
                blurred_texel3,
                blurred_texel4,
            );
//#endif
//#if GLYPH
        } else if paint_type == PAINT_TYPE_GLYPH {
            // A glyph quad lands on whole pixels and its slot holds the glyph
            // already rasterized at the run's subpixel offset, so a fragment
            // centre maps to exactly one atlas texel: the one `textureLoad`
            // the image path's nearest arm would reach through the transform,
            // taken here with no transform and no extend mode at all. The
            // record is the run's, read for its atlas layer and tint alone.
            let paint_tex_idx = paint_and_rect_flag & PAINT_TEXTURE_INDEX_MASK;
            let glyph_texel0 = load_encoded_paint_texel(paint_tex_idx, 0u);
            let glyph_texel2 = load_encoded_paint_texel(paint_tex_idx, 2u);
            let glyph_tint = unpack4x8unorm(glyph_texel2.y);
            let glyph_multiply = glyph_texel2.z == TINT_MODE_MULTIPLY;
            let glyph_color = textureLoad(
                atlas_texture_array,
                vec2<u32>(sample_xy),
                i32(get_image_atlas_index(glyph_texel0)),
                0,
            );
            final_color = alpha * select(
                glyph_tint * glyph_color.a,
                glyph_color * glyph_tint,
                glyph_multiply
            );
//#endif
        }
//#if LAYER
    } else if color_source == COLOR_SOURCE_LAYER {
        let layer_opacity = f32(paint_and_rect_flag & 0xffu) * (1.0 / 255.0);
        final_color = alpha * layer_opacity * textureLoad(layer_input_texture, vec2<i32>(sample_xy), 0);
//#endif
    } else {
        final_color = vec4<f32>(0.0);
    }

    return final_color;
}
