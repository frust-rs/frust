//! WGSL fragment-shader sources for the showcase, plus a small registry.
//!
//! ## Contract
//!
//! Each fragment source below is compiled after `frust-render`'s own fixed
//! prelude (`crates/frust-render/src/shader_effects.rs`'s `VERTEX_PRELUDE`),
//! which already declares the `Uniforms` struct and the fullscreen-triangle
//! vertex stage. A source here therefore must **not** redeclare `Uniforms` or
//! a `@vertex` stage — it only defines the fragment entry point `fs_main`
//! (`@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32>`) and
//! reads the uniforms as `frust_u.resolution` / `frust_u.time`. Every shader
//! here writes **opaque** output (`vec4<f32>(color, 1.0)`) — the v1
//! [`frust_scene::ShaderProgram`] contract (RESEARCH.md §Q1: vello's
//! image-override copy only agrees with Frust's premultiplied render target
//! at alpha = 1.0).

use frust_scene::ShaderProgram;

/// *Neon rings* — a port of
/// [shadertoy.com/view/mtyGWy](https://www.shadertoy.com/view/mtyGWy),
/// including its iquilezles cosine-palette `palette()` helper
/// (<https://iquilezles.org/articles/palettes/>).
///
/// Port notes (`workflow/plans/features/frust-shader-showcase/research/RESEARCH.md`
/// §Q5): `@builtin(position)` is top-left-origin (shadertoy's `fragCoord` is
/// bottom-left), so the y-flip below restores shadertoy parity; the loop
/// counter is an `f32` accumulator like the original; `d = abs(sin(...))`
/// keeps the following `pow(0.01 / d, 1.2)` in its defined domain (`pow` is
/// spec-undefined for a negative base).
const NEON_RINGS_WGSL: &str = r#"
fn palette(t: f32) -> vec3<f32> {
    let a = vec3<f32>(0.5, 0.5, 0.5);
    let b = vec3<f32>(0.5, 0.5, 0.5);
    let c = vec3<f32>(1.0, 1.0, 1.0);
    let d = vec3<f32>(0.263, 0.416, 0.557);
    return a + b * cos(6.28318 * (c * t + d));
}

@fragment
fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {
    let pos = in.position.xy;
    // y-flip: FrustVsOut::position is top-left origin; shadertoy's fragCoord
    // is bottom-left.
    let fc = vec2<f32>(pos.x, frust_u.resolution.y - pos.y);
    var uv = (fc * 2.0 - frust_u.resolution) / frust_u.resolution.y;
    let uv0 = uv;
    var final_color = vec3<f32>(0.0);

    for (var i = 0.0; i < 4.0; i += 1.0) {
        uv = fract(uv * 1.5) - 0.5;

        var d = length(uv) * exp(-length(uv0));
        var col = palette(length(uv0) + i * 0.4 + frust_u.time * 0.4);

        d = abs(sin(d * 8.0 + frust_u.time) / 8.0);
        d = pow(0.01 / d, 1.2);

        final_color += col * d;
    }

    return vec4<f32>(final_color, 1.0);
}
"#;

/// *Palette sweep* — a minimal full-bleed gradient over iquilezles' cosine
/// palette (<https://iquilezles.org/articles/palettes/>), animated by
/// scrolling the palette parameter with time.
const PALETTE_SWEEP_WGSL: &str = r#"
fn palette(t: f32) -> vec3<f32> {
    let a = vec3<f32>(0.5, 0.5, 0.5);
    let b = vec3<f32>(0.5, 0.5, 0.5);
    let c = vec3<f32>(1.0, 1.0, 1.0);
    let d = vec3<f32>(0.263, 0.416, 0.557);
    return a + b * cos(6.28318 * (c * t + d));
}

@fragment
fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {
    let uv = in.position.xy / frust_u.resolution;
    let col = palette(uv.x + frust_u.time * 0.25);
    return vec4<f32>(col, 1.0);
}
"#;

/// Build the showcase's shader registry, in menu display order.
///
/// Called once (`ShadertoyApp::init` — `Component::init` runs once per
/// mount), never per-frame: [`ShaderProgram::new`] mints a fresh
/// process-unique id per call, and `frust-render` compiles/caches a pipeline
/// keyed by that id, so re-minting a program every rebuild would defeat the
/// pipeline cache.
pub fn all() -> Vec<(&'static str, ShaderProgram)> {
    vec![
        ("Neon rings", ShaderProgram::new(NEON_RINGS_WGSL)),
        ("Palette sweep", ShaderProgram::new(PALETTE_SWEEP_WGSL)),
    ]
}
