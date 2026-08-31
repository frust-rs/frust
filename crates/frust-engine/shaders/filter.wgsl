// Copyright 2026 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Derived from vello_sparse_shaders 0.2.0 (`shaders/filter.wesl`); its
// `import package::helpers::...` lines are resolved by prepending
// `helpers.wgsl`, and the blur kernels it calls by prepending
// `filters_blur.wgsl`, at load time (`crate::filters::FILTER`).
//
// One filter pass over one destination page: the vertex stage places the quad
// the pass writes, the fragment stage dispatches on the pass kind and returns
// that texel's filtered colour. A whole Gaussian blur is a sequence of these
// passes, each its own render pass over the page the previous one did not
// write, planned host-side by `crate::filters::blur`.
//
// Only the blur half of the reference's pass set is implemented here. The
// numbering of every pass kind is kept, so the flood, offset and drop-shadow
// arms can be added without renumbering the wire format; a pass kind this
// module does not implement can never reach it, because the scheduler refuses
// every filter but the blur before a pass is ever planned.

// The texture holding the encoded parameters of every filter in the frame.
@group(0) @binding(0)
var filter_data: texture_2d<u32>;
// The page holding this pass's source, one half of the ping-pong pair.
@group(1) @binding(0)
var source_texture: texture_2d<f32>;
// A bilinear sampler over `source_texture`.
@group(1) @binding(1)
var linear_sampler: sampler;

// Keep every constant and layout below in sync with `crate::filters::blur`.

// Every filter's parameter block is this many bytes, whatever its kind, which
// is what lets one texel offset address any of them. Declared here rather than
// used here: `load_filter_texel` is handed an offset the host already expressed
// in texels, and these are the numbers that offset was computed from.
const FILTER_SIZE_BYTES: u32 = 48u;
const FILTER_SIZE_U32: u32 = FILTER_SIZE_BYTES / 4u;
const TEXELS_PER_FILTER: u32 = FILTER_SIZE_U32 / 4u;

// Pass kinds 1, 2, 7 and 8 are the reference's flood, offset and the two
// drop-shadow composites; they are reserved rather than implemented.
const PASS_COPY: u32 = 0u;
const PASS_DOWNSCALE: u32 = 3u;
const PASS_BLUR_H: u32 = 4u;
const PASS_BLUR_V: u32 = 5u;
const PASS_UPSCALE: u32 = 6u;

// Transparent border a decimated pass overdraws around the region it writes.
//
// Half a kernel is as far as a bilinear tap can reach past the region it is
// filtering, so a border of transparent black that wide is what lets the
// kernels skip their bounds checks. The scheduler already clears a filter
// round's destination page, which guarantees it for a page holding one layer;
// the overdraw is what would still guarantee it were a page ever shared.
//
// Keep in sync with `FILTER_ATLAS_PADDING` in `crate::filters::blur`.
const FILTER_ATLAS_PADDING: u32 = 6u;

// The packed header in the first word of a filter's parameter block:
//   bits [0:4]   filter_type       (5 bits)
//   bits [5:6]   edge_mode         (2 bits, blur only; ignored by this module)
//   bits [7:10]  n_decimations     (4 bits, blur only; read host-side only)
//   bits [11:12] n_linear_taps     (2 bits, blur only)
//   bit  [13]    composite_original (drop shadow only; read host-side only)
//   bits [14:31] reserved

// One texel of the filter parameter block starting at `texel_offset`.
fn load_filter_texel(texel_offset: u32, texel_index: u32) -> vec4<u32> {
    let w = textureDimensions(filter_data).x;
    let flat_index = texel_offset + texel_index;
    return textureLoad(filter_data, flat_index_to_texture_coord(flat_index, w), 0);
}

// How many merged bilinear tap pairs per side the blur kernel carries.
fn get_filter_header_n_linear_taps(texel0: vec4<u32>) -> u32 { return (texel0.x >> 11u) & 0x3u; }

// The blur kernel's centre weight.
fn get_blur_center_weight(texel0: vec4<u32>) -> f32 { return bitcast<f32>(texel0.y); }

// The blur kernel's merged tap weights. Assumes `MAX_TAPS_PER_SIDE` is 3.
fn get_blur_linear_weights(texel0: vec4<u32>, texel1: vec4<u32>) -> vec3<f32> {
    return vec3<f32>(
        bitcast<f32>(texel0.z),
        bitcast<f32>(texel0.w),
        bitcast<f32>(texel1.x),
    );
}

// The blur kernel's merged tap offsets. Assumes `MAX_TAPS_PER_SIDE` is 3.
fn get_blur_linear_offsets(texel1: vec4<u32>) -> vec3<f32> {
    return vec3<f32>(
        bitcast<f32>(texel1.y),
        bitcast<f32>(texel1.z),
        bitcast<f32>(texel1.w),
    );
}

// Must stay byte-compatible with `crate::filters::blur::FilterInstanceData`.
struct FilterInstanceData {
    // Origin of this pass's source region in the source page, packed as u16s.
    @location(0)
    source_origin: u32,
    // Extent of this pass's source region, packed as u16s.
    @location(1)
    source_size: u32,
    // Origin of this pass's destination region in the destination page, packed
    // as u16s.
    @location(2)
    dest_origin: u32,
    // Extent of this pass's destination region, packed as u16s.
    @location(3)
    dest_size: u32,
    // Extent of the whole destination page, packed as u16s.
    @location(4)
    dest_texture_size: u32,
    // Texel offset of this filter's parameter block in `filter_data`.
    @location(5)
    filter_data_offset: u32,
    // Extent of the filter layer's unscaled region, packed as u16s.
    @location(6)
    original_size: u32,
    // Which pass of the filter's sequence this instance runs.
    @location(7)
    filter_pass_kind: u32,
}

struct FilterVertexOutput {
    @builtin(position)
    position: vec4<f32>,
    @location(0) @interpolate(flat)
    filter_data_offset: u32,
    @location(1) @interpolate(flat)
    source_origin: vec2<u32>,
    @location(2) @interpolate(flat)
    dest_origin: vec2<u32>,
    @location(3) @interpolate(flat)
    dest_size: vec2<u32>,
    @location(4) @interpolate(flat)
    filter_pass_kind: u32,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    instance: FilterInstanceData,
) -> FilterVertexOutput {
    let source_origin = unpack_u16_pair(instance.source_origin);
    let dest_origin = unpack_u16_pair(instance.dest_origin);
    let dest_size = unpack_u16_pair(instance.dest_size);
    let dest_texture_size = vec2<f32>(unpack_u16_pair(instance.dest_texture_size));
    let original_size = unpack_u16_pair(instance.original_size);

    // A decimated pass writes fewer texels than the pass before it did, so the
    // quad is drawn a padding border wider than the region and the fragment
    // stage writes transparent black over the difference — bounded by the
    // layer's own unscaled extent, past which nothing is ever sampled.
    let render_size = min(original_size, dest_size + vec2(FILTER_ATLAS_PADDING));
    let corner = quad_corner(vertex_index);
    let dest_xy = vec2<f32>(dest_origin) + corner * vec2<f32>(render_size);

    var out: FilterVertexOutput;
    out.position = vec4<f32>(pixel_to_ndc(dest_xy, dest_texture_size), 0.0, 1.0);
    out.filter_data_offset = instance.filter_data_offset;
    out.source_origin = source_origin;
    out.dest_origin = dest_origin;
    out.dest_size = dest_size;
    out.filter_pass_kind = instance.filter_pass_kind;
    return out;
}

// One texel of the source region, addressed relative to its origin.
//
// `rel_coord` is inside the region by construction: the fragment stage returns
// before reaching here for a texel of the padding border.
fn sample_source(source_origin: vec2<u32>, rel_coord: vec2<f32>) -> vec4<f32> {
    let source_coord = vec2<u32>(vec2<i32>(source_origin) + vec2<i32>(rel_coord));
    return textureLoad(source_texture, source_coord, 0);
}

const HORIZONTAL: vec2<f32> = vec2<f32>(1.0, 0.0);
const VERTICAL: vec2<f32> = vec2<f32>(0.0, 1.0);

@fragment
fn fs_main(
    @location(0) @interpolate(flat) filter_data_offset: u32,
    @location(1) @interpolate(flat) source_origin: vec2<u32>,
    @location(2) @interpolate(flat) dest_origin: vec2<u32>,
    @location(3) @interpolate(flat) dest_size: vec2<u32>,
    @location(4) @interpolate(flat) filter_pass_kind: u32,
    @builtin(position) position: vec4<f32>,
) -> @location(0) vec4<f32> {
    let frag_coord = vec2<u32>(position.xy);
    let rel_coord = vec2<f32>(frag_coord - dest_origin);
    // The padding border the vertex stage added: cleared, not filtered.
    if rel_coord.x >= f32(dest_size.x) || rel_coord.y >= f32(dest_size.y) {
        return vec4<f32>(0.0);
    }

    switch filter_pass_kind {
        case PASS_COPY: {
            return sample_source(source_origin, rel_coord);
        }
        case PASS_DOWNSCALE: {
            return filter_downscale(source_texture, linear_sampler, position, source_origin, dest_origin);
        }
        case PASS_BLUR_H: {
            let filter_texel0 = load_filter_texel(filter_data_offset, 0u);
            let filter_texel1 = load_filter_texel(filter_data_offset, 1u);
            return filter_convolve(
                source_texture,
                linear_sampler,
                source_origin,
                rel_coord,
                HORIZONTAL,
                get_filter_header_n_linear_taps(filter_texel0),
                get_blur_center_weight(filter_texel0),
                get_blur_linear_weights(filter_texel0, filter_texel1),
                get_blur_linear_offsets(filter_texel1),
            );
        }
        case PASS_BLUR_V: {
            let filter_texel0 = load_filter_texel(filter_data_offset, 0u);
            let filter_texel1 = load_filter_texel(filter_data_offset, 1u);
            return filter_convolve(
                source_texture,
                linear_sampler,
                source_origin,
                rel_coord,
                VERTICAL,
                get_filter_header_n_linear_taps(filter_texel0),
                get_blur_center_weight(filter_texel0),
                get_blur_linear_weights(filter_texel0, filter_texel1),
                get_blur_linear_offsets(filter_texel1),
            );
        }
        case PASS_UPSCALE: {
            return filter_upscale(source_texture, linear_sampler, position, source_origin, dest_origin);
        }
        // A pass kind this module does not implement; unreachable, because the
        // scheduler plans none.
        default: {
            return vec4<f32>(0.0);
        }
    }
}
