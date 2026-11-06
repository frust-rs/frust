// The GPU pass section's shader: one textured quad, rotated about the
// target's centre. Render-only (no compute); output is premultiplied.

struct Params {
    // Rotation in radians.
    angle: f32,
    // Target width / height, so the quad stays square under rotation.
    aspect: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var checker: texture_2d<f32>;
@group(0) @binding(2) var checker_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    // Two triangles covering the unit square [-1, 1]^2.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let corner = corners[index];
    let half_extent = 0.55;
    let c = cos(params.angle);
    let s = sin(params.angle);
    let rotated = vec2<f32>(
        corner.x * c - corner.y * s,
        corner.x * s + corner.y * c,
    ) * half_extent;

    var out: VsOut;
    // Divide x by the aspect ratio: a square in pixels, not in clip space.
    out.position = vec4<f32>(rotated.x / params.aspect, rotated.y, 0.0, 1.0);
    out.uv = corner * 0.5 + vec2<f32>(0.5, 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(checker, checker_sampler, in.uv);
    // Premultiplied: the engine composites a pass's target blended.
    return vec4<f32>(texel.rgb * texel.a, texel.a);
}
