// Copyright 2025 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Derived from vello_sparse_shaders 0.2.0 (`shaders/clear.wesl`); its
// `import package::helpers::geometry::...` line is resolved by prepending
// `helpers.wgsl` at load time (`gpu::shader_src`).
//
// Clears rectangular regions of an intermediate texture to transparent. The
// module declares no bindings at all: the region comes from either an
// instance buffer (`vs_main`) or the render pass's scissor rect
// (`vs_main_fullscreen`).

// Clears an explicit region, one instance per rectangle. Must stay
// byte-compatible with the clear-instance vertex layout in `gpu::pipelines`.
@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) origin: vec2<u32>,
    @location(1) size: vec2<u32>,
    @location(2) target_size: vec2<u32>,
) -> @builtin(position) vec4<f32> {
    let corner = quad_corner(vertex_index);
    let pixel = vec2<f32>(origin) + corner * vec2<f32>(size);
    let ndc = pixel_to_ndc(pixel, vec2<f32>(target_size));
    return vec4<f32>(ndc, 0.0, 1.0);
}

// Clears an atlas region. This generates a quad covering the whole render
// target; the region actually cleared is controlled by the scissor test set
// on the render pass. That is cheaper than generating region-specific
// geometry:
// 1. no vertex buffer is needed — the geometry comes from the vertex index;
// 2. the same shader works for any region size;
// 3. the hardware scissor test clips to the region;
// 4. rasterizer plus scissor beats per-region vertex math.
@vertex
fn vs_main_fullscreen(
    @builtin(vertex_index) vertex_index: u32,
) -> @builtin(position) vec4<f32> {
    // Map vertex_index (0-3) to fullscreen quad corners in NDC:
    // 0 -> (-1,-1), 1 -> (1,-1), 2 -> (-1,1), 3 -> (1,1)
    let ndc = quad_corner(vertex_index) * 2.0 - vec2(1.0);
    return vec4<f32>(ndc, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Clear with transparent pixels.
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}
