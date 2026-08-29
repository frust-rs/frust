// Copyright 2026 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Derived from vello_sparse_shaders 0.2.0 (`shaders/copy.wesl`); its
// `import package::helpers::...` lines are resolved by prepending
// `helpers.wgsl` at load time (`gpu::shader_src`).
//
// Copies rectangular regions between intermediate textures, one instance per
// region.

// Must stay byte-compatible with the copy-instance vertex layout in
// `gpu::pipelines`.
struct CopyInstance {
    // Origin in the destination texture page, packed as u16s.
    @location(0)
    dest_texture_origin: u32,
    // Origin in the source texture page, packed as u16s.
    @location(1)
    source_texture_origin: u32,
    // Width and height of the copied region, packed as u16s.
    @location(2)
    copy_rect_size: u32,
    // Width and height of the destination texture page, packed as u16s.
    @location(3)
    dest_texture_size: u32,
}

struct VertexOutput {
    @location(0)
    source_texture_xy: vec2<f32>,
    @builtin(position)
    position: vec4<f32>,
}

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    instance: CopyInstance,
) -> VertexOutput {
    let dest_texture_origin = unpack_u16_pair(instance.dest_texture_origin);
    let source_texture_origin = unpack_u16_pair(instance.source_texture_origin);
    let copy_rect_size = unpack_u16_pair(instance.copy_rect_size);
    let dest_texture_size = unpack_u16_pair(instance.dest_texture_size);

    let local = quad_corner(vertex_index) * vec2<f32>(copy_rect_size);
    let dest_texture_xy = vec2<f32>(dest_texture_origin) + local;

    var out: VertexOutput;
    out.source_texture_xy = vec2<f32>(source_texture_origin) + local;

    let ndc = pixel_to_ndc(dest_texture_xy, vec2<f32>(dest_texture_size));
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(
    @location(0) source_texture_xy: vec2<f32>,
) -> @location(0) vec4<f32> {
    return textureLoad(source_texture, vec2<i32>(source_texture_xy), 0);
}
