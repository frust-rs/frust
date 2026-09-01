// Frust's own program, not a port of a `vello_sparse_shaders` module — which
// is why it carries no upstream attribution header and pulls in no helper
// prelude (`gpu::shader_src`): it generates its own geometry and reads exactly
// one texture.
//
// Converts a PREMULTIPLIED colour buffer into a STRAIGHT-alpha one, for a
// swapchain that stores straight alpha (iOS's `PostMultiplied` composite alpha
// mode). Every engine pipeline blends and writes premultiplied, so the frame
// arrives here as `(C·a, a)` and such a compositor would read it as `(C, a)` —
// every partial-alpha pixel too dark. One full-screen triangle over the whole
// destination writes `(rgb / max(a, ALPHA_FLOOR), a)` instead.
//
// A render pass, deliberately: the destination is a swapchain image that is
// `RENDER_ATTACHMENT`-only, and a compute/storage-texture conversion is
// refused by the engine's downlevel design rules (E1/E2) — the whole tier
// exists to run where compute does not.

// The divisor's floor. `a` at or near zero carries no colour information at
// all (a premultiplied `(0,0,0,0)` is the only value it can hold), so the
// quotient is arbitrary — but an unguarded `0 / 0` is a NaN, and a NaN written
// to the swapchain is undefined content rather than a transparent pixel.
// Flooring the divisor keeps the result finite and, since the numerator is
// zero there, exactly transparent black. Small enough that no representable
// 8-bit alpha (the smallest is 1/255 ≈ 3.9e-3) is ever clamped by it, so the
// guard never perturbs a pixel that carries real colour.
//
// Must stay in step with `gpu::present`'s `ALPHA_FLOOR`, which its own test
// pins against this file.
const ALPHA_FLOOR: f32 = 1e-4;

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

// The full-screen triangle: three vertices at (-1,-1), (-1,3), (3,-1), whose
// interior covers the whole clip volume with one primitive. No vertex buffer
// and no instance data — the pipeline declares no vertex layout at all.
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let index = i32(vertex_index);
    let x = f32(index / 2) * 4.0 - 1.0;
    let y = f32(index & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// Source and destination are the same extent in the same framebuffer space, so
// the fragment's own position is the source texel to read: a `textureLoad` at
// integer coordinates, never a sampled read, so no filtering or coordinate
// convention can shift the image by half a texel.
//
// The quotient is written to a `unorm` target, which clamps it — a premultiplied
// pixel whose rounded `rgb` sits a hair above its own `a` therefore lands at 1.0
// rather than wrapping.
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let premultiplied = textureLoad(source_texture, vec2<i32>(position.xy), 0);
    let alpha = max(premultiplied.a, ALPHA_FLOOR);
    return vec4<f32>(premultiplied.rgb / alpha, premultiplied.a);
}
