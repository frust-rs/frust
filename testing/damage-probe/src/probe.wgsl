// Two solid rectangles from one uniform: instance 0 is the moving square,
// instance 1 the static reference marker. Pixel coordinates, top-left origin —
// the same convention `Queue::present_with_damage` uses for its rects.
struct Uniforms {
    surface: vec2<f32>,
    _pad: vec2<f32>,
    rects: array<vec4<f32>, 2>,
    colors: array<vec4<f32>, 2>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[vi];
    // Constant indices only: dynamic indexing of a uniform array failed naga's
    // GLSL-ES translation on an Adreno 660 (empty error text, Pixel 4a was fine).
    let r = select(u.rects[1], u.rects[0], ii == 0u);
    let px = r.xy + c * r.zw;
    let ndc = vec2<f32>(px.x / u.surface.x * 2.0 - 1.0, 1.0 - px.y / u.surface.y * 2.0);
    var out: VsOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.color = select(u.colors[1], u.colors[0], ii == 0u);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
