//! WGSL fragment-shader sources for the showcase, plus a small registry.
//!
//! ## Contract
//!
//! Each fragment source below is compiled after the fixed prelude from
//! `frust_gpu::effects`, which already declares the `Uniforms` struct and the
//! fullscreen-triangle vertex stage. A source here therefore must **not**
//! redeclare `Uniforms` or a `@vertex` stage — it only defines the fragment
//! entry point `fs_main` (`@fragment fn fs_main(in: FrustVsOut) ->
//! @location(0) vec4<f32>`) and reads the uniforms as `frust_u.resolution` /
//! `frust_u.time`. Every shader here writes **opaque** output
//! (`vec4<f32>(color, 1.0)`) — the v1 [`frust::authoring::scene::ShaderProgram`]
//! contract (vello's image-override copy only agrees with Frust's premultiplied
//! render target at alpha = 1.0).

use frust::authoring::scene::ShaderProgram;

/// *Neon rings* — a port of
/// [shadertoy.com/view/mtyGWy](https://www.shadertoy.com/view/mtyGWy),
/// including its iquilezles cosine-palette `palette()` helper
/// (<https://iquilezles.org/articles/palettes/>).
///
/// Port notes: `@builtin(position)` is top-left-origin (shadertoy's `fragCoord` is
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

/// *Synthwave sunset* — a port of a CC0 neon/synthwave landscape shadertoy
/// (the source header gives no
/// shadertoy URL, just "CC0: For the neon style enjoyers").
///
/// Per-function licenses carried over from the original:
/// - `hsv2rgb`: WTFPL, author sam hocevar (<https://stackoverflow.com/a/17897228/418488>)
/// - `atan_approx`: MIT, author Pascal Gilcher (<https://www.shadertoy.com/view/flSXRV>)
/// - `srgb`: author nmz (twitter: @stormoid), license unknown (<https://www.shadertoy.com/view/NdfyRM>)
/// - `aces_approx`: author Matt Taylor, license unknown (<https://64.github.io/tonemapping/>)
/// - `mod1`/`mod2`: MIT OR CC-BY-NC-4.0, author mercury (<https://mercury.sexy/hg_sdf/>)
/// - `ray_plane`/`sd_box`/`segment`/`ray_sphere`/`vnoise`: MIT, author Inigo
///   Quilez (<https://iquilezles.org>)
///
/// Dead-code note: the original's `rgb2hsv` function and `sunCol1` constant
/// are defined but never called/read there either — omitted here (no
/// behavior change, just less unreachable WGSL). Likewise a handful of
/// write-only locals (`tp` in `skyRender`, `op`/`gp` in `triRender`) are
/// dropped for the same reason.
///
/// Port notes (this crate's WGSL porting rules):
/// - Every `mod`/`mod1`/`mod2` call is floor-mod (`mod_f`/`mod_v2` below, not
///   WGSL's trunc-mod `%`) — the fractal (`city_of_kali`), the ground grid,
///   and the mountain silhouette all wrap negative coordinates.
/// - The `inout float`/`inout vec2` params (`mod1`, `mod2`, and the `maxt`
///   threaded through `tri_render`/`mountain_render`/`ground_render`) become
///   `ptr<function, ...>` params.
/// - The `HSV2RGB`-macro module-scope consts are hand-precomputed to numeric
///   `vec3<f32>` literals (a WGSL module `const` can't call a user
///   function); each keeps its original HSV triplet in a trailing comment.
///   `sunDir`/`sunDir2` (`normalize(...)`) are precomputed the same way to
///   sidestep any question of whether `normalize` const-evaluates at module
///   scope.
/// - `mat2(a,b,c,d)` is column-major in both GLSL and WGSL; `ROT`'s two call
///   sites (`psp.yz *= ROT(...)`, `psp.xy *= ROT(...)`) are GLSL
///   *row*-vector-times-matrix ops (`v*M = (dot(v,col0), dot(v,col1))` per
///   the GLSL spec), ported via the explicit `vec2_mul_mat2` helper below
///   rather than leaning on an assumed WGSL `vec*mat` sign convention.
/// - The y-flip (`fs_main` recovers a bottom-left-origin `fragCoord` from a
///   top-left-origin `@builtin(position)`) flips `mountain_render`'s
///   `dFdy(d) < 0.0` silhouette test to `dpdy(d) > 0.0`.
/// - `THAT_CRT_FEELING` is `#define`-disabled in the source — omitted, along
///   with the `effect()` `pp`/`aa` locals it alone consumed.
const SYNTHWAVE_WGSL: &str = r#"
const PI: f32 = 3.141592654;
const PI_2: f32 = 0.5 * PI;
const TAU: f32 = 2.0 * PI;

const HSV2RGB_K: vec4<f32> = vec4<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);

// Precomputed HSV2RGB(...) module consts (original HSV triplet in the
// trailing comment) — see the module doc's port-notes bullet above.
const SKY_COL: vec3<f32> = vec3<f32>(0.14, 0.5872, 1.0); // HSV2RGB(0.58, 0.86, 1.0)
const SPE_COL1: vec3<f32> = vec3<f32>(0.75, 0.85, 1.0); // HSV2RGB(0.60, 0.25, 1.0)
const SPE_COL2: vec3<f32> = vec3<f32>(0.75, 0.925, 1.0); // HSV2RGB(0.55, 0.25, 1.0)
const DIFF_COL1: vec3<f32> = vec3<f32>(0.10, 0.46, 1.0); // HSV2RGB(0.60, 0.90, 1.0)
const DIFF_COL2: vec3<f32> = vec3<f32>(0.10, 0.73, 1.0); // HSV2RGB(0.55, 0.90, 1.0)
const SUN_DIR2: vec3<f32> = vec3<f32>(0.0, 0.63407959, 0.77326779); // normalize(vec3(0.0, 0.82, 1.0))
const SUN_DIR: vec3<f32> = vec3<f32>(0.0, 0.04993762, 0.99875234); // normalize(vec3(0.0, 0.05, 1.0))
const SUN_COL: vec3<f32> = vec3<f32>(0.00007, 0.0002936, 0.0005); // HSV2RGB(0.58, 0.86, 0.0005)
const MOUNTAIN_POS: f32 = -20.0;

// GLSL `mod` is floor-mod; WGSL `%` is trunc-mod for floats — every `mod`
// call in this shader routes through these instead.
fn mod_f(x: f32, y: f32) -> f32 {
    return x - y * floor(x / y);
}

fn mod_v2(x: vec2<f32>, y: vec2<f32>) -> vec2<f32> {
    return x - y * floor(x / y);
}

fn sca(a: f32) -> vec2<f32> {
    return vec2<f32>(sin(a), cos(a));
}

fn rot(a: f32) -> mat2x2<f32> {
    return mat2x2<f32>(vec2<f32>(cos(a), sin(a)), vec2<f32>(-sin(a), cos(a)));
}

// GLSL's `v *= M` for a row vector `v` computes `(dot(v,col0(M)),
// dot(v,col1(M)))` — see the module doc's `ROT` port-notes bullet.
fn vec2_mul_mat2(v: vec2<f32>, m: mat2x2<f32>) -> vec2<f32> {
    return vec2<f32>(dot(v, m[0]), dot(v, m[1]));
}

// License: WTFPL, author: sam hocevar, found: https://stackoverflow.com/a/17897228/418488
fn hsv2rgb(c: vec3<f32>) -> vec3<f32> {
    let p = abs(fract(c.xxx + HSV2RGB_K.xyz) * 6.0 - HSV2RGB_K.www);
    return c.z * mix(HSV2RGB_K.xxx, clamp(p - HSV2RGB_K.xxx, vec3<f32>(0.0), vec3<f32>(1.0)), c.y);
}

// License: MIT, author: Pascal Gilcher, found: https://www.shadertoy.com/view/flSXRV
fn atan_approx(y: f32, x: f32) -> f32 {
    let cosatan2 = x / (abs(x) + abs(y));
    let t = PI_2 - cosatan2 * PI_2;
    return select(t, -t, y < 0.0);
}

// License: Unknown, author: Unknown, found: don't remember
fn tanh_approx(x: f32) -> f32 {
    let x2 = x * x;
    return clamp(x * (27.0 + x2) / (27.0 + 9.0 * x2), -1.0, 1.0);
}

fn to_spherical(p: vec3<f32>) -> vec3<f32> {
    let r = length(p);
    let t = acos(p.z / r);
    let ph = atan_approx(p.y, p.x);
    return vec3<f32>(r, t, ph);
}

// License: Unknown, author: nmz (twitter: @stormoid), found: https://www.shadertoy.com/view/NdfyRM
fn srgb(t: vec3<f32>) -> vec3<f32> {
    return mix(1.055 * pow(t, vec3<f32>(1.0 / 2.4)) - 0.055, 12.92 * t, step(t, vec3<f32>(0.0031308)));
}

// License: Unknown, author: Matt Taylor (https://github.com/64), found: https://64.github.io/tonemapping/
fn aces_approx(v_in: vec3<f32>) -> vec3<f32> {
    var v = max(v_in, vec3<f32>(0.0));
    v *= 0.6;
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((v * (a * v + b)) / (v * (c * v + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// License: MIT OR CC-BY-NC-4.0, author: mercury, found: https://mercury.sexy/hg_sdf/
fn mod1(p: ptr<function, f32>, size: f32) -> f32 {
    let halfsize = size * 0.5;
    let c = floor((*p + halfsize) / size);
    *p = mod_f(*p + halfsize, size) - halfsize;
    return c;
}

// License: MIT OR CC-BY-NC-4.0, author: mercury, found: https://mercury.sexy/hg_sdf/
fn mod2(p: ptr<function, vec2<f32>>, size: vec2<f32>) -> vec2<f32> {
    let c = floor((*p + size * 0.5) / size);
    *p = mod_v2(*p + size * 0.5, size) - size * 0.5;
    return c;
}

// License: MIT, author: Inigo Quilez, found: https://iquilezles.org/www/articles/intersectors/intersectors.htm
fn ray_plane(ro: vec3<f32>, rd: vec3<f32>, p: vec4<f32>) -> f32 {
    return -(dot(ro, p.xyz) + p.w) / dot(rd, p.xyz);
}

// License: MIT, author: Inigo Quilez, found: https://iquilezles.org/www/articles/distfunctions2d/distfunctions2d.htm
fn equilateral_triangle(p_in: vec2<f32>) -> f32 {
    let k = sqrt(3.0);
    var p = p_in;
    p.x = abs(p.x) - 1.0;
    p.y = p.y + 1.0 / k;
    if (p.x + k * p.y > 0.0) {
        p = vec2<f32>(p.x - k * p.y, -k * p.x - p.y) / 2.0;
    }
    p.x -= clamp(p.x, -2.0, 0.0);
    return -length(p) * sign(p.y);
}

// License: MIT, author: Inigo Quilez, found: https://iquilezles.org/www/articles/distfunctions2d/distfunctions2d.htm
fn sd_box(p: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = abs(p) - b;
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
}

// License: MIT, author: Inigo Quilez, found: https://iquilezles.org/www/articles/distfunctions2d/distfunctions2d.htm
fn segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

// License: Unknown, author: Unknown, found: don't remember
fn hash(co: vec2<f32>) -> f32 {
    return fract(sin(dot(co.xy, vec2<f32>(12.9898, 58.233))) * 13758.5453);
}

// License: MIT, author: Inigo Quilez, found: https://www.shadertoy.com/view/XslGRr
fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);

    let u = f * f * (3.0 - 2.0 * f);

    let a = hash(i + vec2<f32>(0.0, 0.0));
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));

    let m0 = mix(a, b, u.x);
    let m1 = mix(c, d, u.x);
    return mix(m0, m1, u.y);
}

// License: MIT, author: Inigo Quilez, found: https://www.iquilezles.org/www/articles/spherefunctions/spherefunctions.htm
fn ray_sphere(ro: vec3<f32>, rd: vec3<f32>, dim: vec4<f32>) -> vec2<f32> {
    let ce = dim.xyz;
    let ra = dim.w;
    let oc = ro - ce;
    let b = dot(oc, rd);
    let c = dot(oc, oc) - ra * ra;
    var h = b * b - c;
    if (h < 0.0) {
        return vec2<f32>(-1.0);
    }
    h = sqrt(h);
    return vec2<f32>(-b - h, -b + h);
}

fn sky_render(ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    var col = vec3<f32>(0.0);
    col += 0.025 * SKY_COL;
    col += SKY_COL * 0.0033 / pow(1.001 + dot(SUN_DIR2, rd), 2.0);

    let tp0 = ray_plane(ro, rd, vec4<f32>(vec3<f32>(0.0, 1.0, 0.0), 4.0));
    let tp1 = ray_plane(ro, rd, vec4<f32>(vec3<f32>(0.0, -1.0, 0.0), 6.0));

    if (tp1 > 0.0) {
        let pos = ro + tp1 * rd;
        let pp = pos.xz;
        let db = sd_box(pp, vec2<f32>(5.0, 9.0)) - 3.0;

        col += vec3<f32>(4.0) * SKY_COL * rd.y * rd.y * smoothstep(0.25, 0.0, db);
        col += vec3<f32>(0.8) * SKY_COL * exp(-0.5 * max(db, 0.0));
        col += 0.25 * sqrt(SKY_COL) * max(-db, 0.0);
    }

    if (tp0 > 0.0) {
        let pos = ro + tp0 * rd;
        let pp = pos.xz;
        let ds = length(pp) - 0.5;

        col += 0.25 * SKY_COL * exp(-0.5 * max(ds, 0.0));
    }

    return clamp(col, vec3<f32>(0.0), vec3<f32>(10.0));
}

fn sphere(ro: vec3<f32>, rd: vec3<f32>, sdim: vec4<f32>) -> vec4<f32> {
    let si = ray_sphere(ro, rd, sdim);

    let nsp = ro + rd * si.x;

    let light_pos1 = vec3<f32>(0.0, 10.0, 10.0);
    let light_pos2 = vec3<f32>(0.0, -80.0, 10.0);

    let nld1 = normalize(light_pos1 - nsp);
    let nld2 = normalize(light_pos2 - nsp);

    let nnor = normalize(nsp - sdim.xyz);

    let nref = reflect(rd, nnor);

    let sf = 4.0;
    var ndif1 = max(dot(nld1, nnor), 0.0);
    ndif1 *= ndif1;
    let nspe1 = pow(SPE_COL1 * max(dot(nld1, nref), 0.0), sf * vec3<f32>(1.0, 0.8, 0.5));

    var ndif2 = max(dot(nld2, nnor), 0.0);
    ndif2 *= ndif2;
    let nspe2 = pow(SPE_COL2 * max(dot(nld2, nref), 0.0), sf * vec3<f32>(0.9, 0.5, 0.5));

    let nsky = sky_render(nsp, nref);
    var nfre = 1.0 + dot(rd, nnor);
    nfre *= nfre;

    var scol = vec3<f32>(0.0);
    scol += nsky * mix(vec3<f32>(0.25), vec3<f32>(0.5, 0.5, 1.0), nfre);
    scol += DIFF_COL1 * ndif1;
    scol += DIFF_COL2 * ndif2;
    scol += nspe1;
    scol += nspe2;

    let t = tanh_approx(2.0 * (si.y - si.x) / sdim.w);

    return vec4<f32>(scol, t);
}

fn sphere_render(ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    let sky_col_local = sky_render(ro, rd);
    var col = sky_col_local;
    let sdim0 = vec4<f32>(vec3<f32>(0.0), 2.0);
    let scol0 = sphere(ro, rd, sdim0);
    col = mix(col, scol0.xyz, scol0.w);
    return col;
}

fn sphere_effect(p: vec2<f32>) -> vec3<f32> {
    let fov = tan(TAU / 6.0);
    let ro = vec3<f32>(0.0, 2.0, 5.0);
    let la = vec3<f32>(0.0, 0.0, 0.0);
    let up = vec3<f32>(0.0, 1.0, 0.0);

    let ww = normalize(la - ro);
    let uu = normalize(cross(up, ww));
    let vv = cross(ww, uu);
    let rd = normalize(-p.x * uu + p.y * vv + fov * ww);

    return sphere_render(ro, rd);
}

fn city_of_kali(p: vec2<f32>) -> vec3<f32> {
    let c = -vec2<f32>(0.5, 0.5) * 1.12;

    var s = 2.0;
    var kp = p / s;

    let a = PI / 4.0;
    let n = vec2<f32>(cos(a), sin(a));

    var ot2 = 1e6;
    var ot3 = 1e6;
    var n2 = 0.0;
    var n3 = 0.0;

    let mx = 12.0;
    for (var i: f32 = 0.0; i < mx; i += 1.0) {
        let m = dot(kp, kp);
        s *= m;
        kp = abs(kp) / m + c;
        let d2 = abs(dot(kp, n)) * s;
        if (d2 < ot2) {
            n2 = i;
            ot2 = d2;
        }
        let d3 = dot(kp, kp);
        if (d3 < ot3) {
            n3 = i;
            ot3 = d3;
        }
    }
    var col = vec3<f32>(0.0);
    n2 /= mx;
    n3 /= mx;
    col += 0.25 * (hsv2rgb(vec3<f32>(0.8 - 0.2 * n2 * n2, 0.90, 0.025)) / (sqrt(ot2) + 0.0025));
    col += hsv2rgb(vec3<f32>(0.55 + 0.8 * n3, 0.85, 0.00000025)) / (ot3 * ot3 + 0.000000025);
    return col;
}

fn outer_sky_render(ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    let center = ro + vec3<f32>(-100.0, 40.0, 100.0);
    let sdim = vec4<f32>(center, 50.0);
    let pi = ray_sphere(ro, rd, sdim);
    let pn = normalize(vec3<f32>(0.0, 1.0, -0.8));
    let pdim = vec4<f32>(pn, -dot(pn, center));
    let ri = ray_plane(ro, rd, pdim);

    var col = vec3<f32>(0.0);

    col += SUN_COL / pow(1.001 - dot(SUN_DIR, rd), 2.0);

    if (pi.x != -1.0) {
        let pp = ro + rd * pi.x;
        var psp = pp - sdim.xyz;
        let sp_n = normalize(pp - sdim.xyz);
        psp = psp.zxy;
        let rotated_yz = vec2_mul_mat2(vec2<f32>(psp.y, psp.z), rot(-0.5));
        psp = vec3<f32>(psp.x, rotated_yz.x, rotated_yz.y);
        let rotated_xy = vec2_mul_mat2(vec2<f32>(psp.x, psp.y), rot(0.025 * frust_u.time));
        psp = vec3<f32>(rotated_xy.x, rotated_xy.y, psp.z);
        let pss = to_spherical(psp);
        var pcol = vec3<f32>(0.0);
        let dif = max(dot(sp_n, SUN_DIR), 0.0);
        let sc = 2000.0 * SUN_COL;
        pcol += sc * dif;
        pcol += city_of_kali(pss.yz) * smoothstep(0.125, 0.0, dif);
        pcol += pow(max(dot(reflect(rd, sp_n), SUN_DIR), 0.0), 9.0) * sc;
        col = mix(col, pcol, tanh_approx(0.125 * (pi.y - pi.x)));
    }

    var gcol = vec3<f32>(0.0);

    let rp = ro + rd * ri;
    let rl = length(rp - center);
    let rb = 1.55 * sdim.w;
    let re = 2.45 * sdim.w;
    let rw = 0.1 * sdim.w;
    let rcol = hsv2rgb(vec3<f32>(clamp(0.005 * (rl + 32.0), 0.6, 0.8), 0.9, 1.0));
    gcol = rcol * 0.025;
    if (ri > 0.0 && (pi.x == -1.0 || ri < pi.x)) {
        var mrl = rl;
        let nrl = mod1(&mrl, rw);
        let rfre = 1.0 + dot(rd, pn);
        var rrcol = rcol / max(abs(mrl), 0.1 + smoothstep(0.7, 1.0, rfre));
        rrcol *= smoothstep(1.0, 0.3, rfre);
        rrcol *= smoothstep(re, re - 0.5 * rw, rl);
        rrcol *= smoothstep(rb - 0.5 * rw, rb, rl);
        col += rrcol;
        _ = nrl; // unused past this point in the original too
    }

    col += gcol / max(abs(rd.y), 0.0033);

    return col;
}

fn tri_render(col_in: vec3<f32>, ro: vec3<f32>, rd: vec3<f32>, maxt: ptr<function, f32>) -> vec3<f32> {
    let tpn = normalize(vec3<f32>(0.0, 0.0, 1.0));
    let tpdim = vec4<f32>(tpn, -2.0);
    let tpd = ray_plane(ro, rd, tpdim);

    if (tpd < 0.0 || tpd > *maxt) {
        return col_in;
    }

    let pp = ro + rd * tpd;
    var p = pp.xy;
    p *= 0.5;

    let off = 1.2 - 0.02;
    p.y -= off;
    let n = sca(-PI / 3.0);
    let hoff = 0.15 * dot(n, p);
    let gcol = hsv2rgb(vec3<f32>(clamp(0.7 + hoff, 0.6, 0.8), 0.90, 0.02));
    var pt = p;
    pt.y = -pt.y;
    let zt = 1.0;
    let dt = equilateral_triangle(pt / zt) * zt;

    var col = col_in;
    if (dt < 0.0) {
        col = sphere_effect(1.5 * p);
    }
    col += (gcol / max(abs(dt), 0.001)) * smoothstep(0.25, 0.0, dt);
    if (dt < 0.0) {
        *maxt = tpd;
    }
    return col;
}

fn height_factor(p: vec2<f32>) -> f32 {
    return 4.0 * smoothstep(7.0, 0.5, abs(p.x)) + 0.5;
}

fn hifbm(p_in: vec2<f32>) -> f32 {
    var p = p_in * 0.25;
    let hf = height_factor(p);
    let aa = 0.5;
    let pp = 2.0 - 0.0;

    var sum = 0.0;
    var a = 1.0;

    for (var i: i32 = 0; i < 5; i += 1) {
        sum += a * vnoise(p);
        a *= aa;
        p *= pp;
    }

    return hf * sum;
}

fn hiheight(p: vec2<f32>) -> f32 {
    return hifbm(p);
}

fn lofbm(p_in: vec2<f32>) -> f32 {
    var p = p_in * 0.25;
    let hf = height_factor(p);
    let aa = 0.5;
    let pp = 2.0 - 0.0;

    var sum = 0.0;
    var a = 1.0;

    for (var i: i32 = 0; i < 3; i += 1) {
        sum += a * vnoise(p);
        a *= aa;
        p *= pp;
    }

    return hf * sum;
}

fn loheight(p: vec2<f32>) -> f32 {
    return lofbm(p) - 0.5;
}

fn mountain_render(col_in: vec3<f32>, ro: vec3<f32>, rd: vec3<f32>, flip: bool, maxt: ptr<function, f32>) -> vec3<f32> {
    let tpn = normalize(vec3<f32>(0.0, 0.0, 1.0));
    let tpdim = vec4<f32>(tpn, MOUNTAIN_POS);
    let tpd = ray_plane(ro, rd, tpdim);

    if (tpd < 0.0 || tpd > *maxt) {
        return col_in;
    }

    let pp = ro + rd * tpd;
    let p = pp.xy;
    let cw = 1.0 - 0.25;
    let hz = 0.0 * frust_u.time + 1.0;
    let lo = loheight(vec2<f32>(p.x, hz));
    var cpx = p.x;
    let cn = mod1(&cpx, cw);
    let cp = vec2<f32>(cpx, p.y);

    let reps = 1.0;
    var d = 1e3;

    for (var i: f32 = -reps; i <= reps; i += 1.0) {
        let x0 = (cn - 0.5 + i) * cw;
        let x1 = (cn - 0.5 + (i + 1.0)) * cw;

        let y0 = hiheight(vec2<f32>(x0, hz));
        let y1 = hiheight(vec2<f32>(x1, hz));

        let dd = segment(cp, vec2<f32>(-cw * 0.5 + cw * i, y0), vec2<f32>(cw * 0.5 + cw * i, y1));
        d = min(d, dd);
    }

    let rcol = hsv2rgb(vec3<f32>(clamp(0.7 + (0.5 * rd.x), 0.6, 0.8), 0.95, 0.125));
    let sd = 1.0001 - dot(SUN_DIR, rd);

    var col = col_in;
    let aa = fwidth(p.y);
    // y-flip: `dFdy(d) < 0.0` in the original becomes `dpdy(d) > 0.0` here —
    // see the module doc's y-flip port-notes bullet.
    if ((dpdy(d) > 0.0) == !flip) {
        col *= mix(0.0, 1.0, smoothstep(aa, -aa, d - aa));
        col += hsv2rgb(vec3<f32>(0.55, 0.85, 0.8)) * smoothstep(0.0, 5.0, lo - p.y);
        *maxt = tpd;
    }
    col += 3.0 * rcol / (abs(d) + 0.005 + 800.0 * sd * sd * sd * sd);
    col += hsv2rgb(vec3<f32>(0.55, 0.96, 0.075)) / (abs(p.y) + 0.05);

    return col;
}

fn ground_render(col_in: vec3<f32>, ro: vec3<f32>, rd: vec3<f32>, maxt: ptr<function, f32>) -> vec3<f32> {
    let gpn = normalize(vec3<f32>(0.0, 1.0, 0.0));
    let gpdim = vec4<f32>(gpn, 0.0);
    let gpd = ray_plane(ro, rd, gpdim);

    if (gpd < 0.0) {
        return col_in;
    }

    *maxt = gpd;

    let gp = ro + rd * gpd;
    var gpfre = 1.0 + dot(rd, gpn);
    gpfre *= gpfre;
    gpfre *= gpfre;
    gpfre *= gpfre;

    let grr = reflect(rd, gpn);

    var ggp = gp.xz;
    ggp.y += frust_u.time;
    // y-flip: a positive grid width needs the flip-corrected magnitude, not
    // the raw (possibly sign-flipped) derivative — see the module doc.
    let dfy = abs(dpdy(ggp.y));
    let gcf = sin(ggp.x) * sin(ggp.y);
    _ = mod2(&ggp, vec2<f32>(1.0));
    let ggd = min(abs(ggp.x), abs(ggp.y));

    let gcol = hsv2rgb(vec3<f32>(0.7 + 0.1 * gcf, 0.90, 0.02));

    var rmaxt = 1e6;
    var rcol = outer_sky_render(gp, grr);
    rcol = mountain_render(rcol, gp, grr, true, &rmaxt);
    rcol = tri_render(rcol, gp, grr, &rmaxt);

    var col = gcol / max(ggd, 0.0 + 0.25 * dfy) * exp(-0.25 * gpd);
    rcol += hsv2rgb(vec3<f32>(0.65, 0.85, 1.0)) * gpfre;
    rcol = 4.0 * tanh(rcol * 0.25);
    col += rcol * gpfre;

    return col;
}

fn render(ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    var maxt = 1e6;

    var col = outer_sky_render(ro, rd);
    col = ground_render(col, ro, rd, &maxt);
    col = mountain_render(col, ro, rd, false, &maxt);
    col = tri_render(col, ro, rd, &maxt);

    return col;
}

fn effect(p: vec2<f32>) -> vec3<f32> {
    let fov = tan(TAU / 6.0);
    let ro = vec3<f32>(0.0, 1.0, -4.0);
    let la = vec3<f32>(0.0, 1.0, 0.0);
    let up = vec3<f32>(0.0, 1.0, 0.0);

    let ww = normalize(la - ro);
    let uu = normalize(cross(up, ww));
    let vv = cross(ww, uu);
    let rd = normalize(-p.x * uu + p.y * vv + fov * ww);

    var col = render(ro, rd);
    // THAT_CRT_FEELING is `#define`-disabled in the original — omitted here,
    // along with its sole consumers (the `pp`/`aa` locals).
    col -= 0.05 * vec3<f32>(2.0, 1.0, 0.0);
    col = aces_approx(col);
    col = srgb(col);
    return col;
}

@fragment
fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {
    let pos = in.position.xy;
    // y-flip: FrustVsOut::position is top-left origin; shadertoy's fragCoord
    // is bottom-left.
    let fc = vec2<f32>(pos.x, frust_u.resolution.y - pos.y);
    let q = fc / frust_u.resolution;

    var p = -1.0 + 2.0 * q;
    p.x *= frust_u.resolution.x / frust_u.resolution.y;
    let col = effect(p);

    return vec4<f32>(col, 1.0);
}
"#;

/// *Glassy field* — a port of Shane's ["Abstract Glassy
/// Field"](https://www.shadertoy.com/view/WlSSzK).
/// No explicit license header in
/// the source; shadertoy's default license is CC BY-NC-SA 3.0.
///
/// **Documented visual deviation:** v1 has no `iChannel`
/// texture binding, so the original's two texture lookups are substituted
/// procedurally, both built on the shader's own `n3D` value-noise function:
/// - `tpl` (tri-planar texture blend) samples a procedural `tex_noise`
///   (three octaves of `n3d`, tinted toward a pale fog/glass color) in place
///   of each `texture(t, uv)` lookup, keeping the original's tri-planar
///   normal-weighting (`n = max(abs(n)-.2,.001); n /= dot(n,vec3(1))`).
/// - `envMap` keeps its `smoothstep(0,1,tpl(...))` shape, just fed the
///   procedural `tpl` above instead of `iChannel1`.
/// - **The texture bump-mapping (`db`) is dropped**, using the un-bumped
///   surface normal (`svn`) directly — the rule's implementor's-call
///   option. The procedural noise amplified through `db`'s `1/e.x` (≈1000×)
///   gradient scale produced a visibly noisy, high-frequency shimmer, and
///   `db`'s only other input (`iChannel0`) has no real texture to bump
///   against here anyway — the geometric shape and glass/glow shading read
///   fine without it. This is a candidate for follow-up refinement, not a
///   blocker.
///
/// Dead-code note: the original's `rot2` helper (Fabrice Neyret's rotation
/// trick, credited in its own comment) is defined but never called there
/// either — omitted here.
///
/// Port notes (this crate's WGSL porting rules):
/// - Every `mod` call (the field's sinusoidal wrap in `map`, the two hash
///   terms in `n3D`, the purple electric-charge modulus in `mainImage`) is
///   floor-mod (`mod_f`/`mod_v3`/`mod_v4` below), not WGSL's trunc-mod `%`.
/// - GLSL's global `float accum` (mutated across `trace`'s raymarch loop and
///   read back in `mainImage`) becomes a WGSL module-scope `var<private>` —
///   the private address space is exactly per-invocation mutable state.
/// - A few 2-component swizzle *write* targets in the original
///   (`p.xy -= ...`, `h.xy = ...`) are rewritten as whole-vector
///   reconstructions instead, sidestepping any question of multi-component
///   swizzle-assignment support.
const GLASSY_FIELD_WGSL: &str = r#"
const FAR: f32 = 50.0;

// Accumulated raymarch glow (per the port notes above) — mutated by
// `trace`, read back in `fs_main`.
var<private> accum: f32 = 0.0;

// GLSL `mod` is floor-mod; WGSL `%` is trunc-mod for floats — every `mod`
// call in this shader routes through one of these instead.
fn mod_f(x: f32, y: f32) -> f32 {
    return x - y * floor(x / y);
}

fn mod_v3(x: vec3<f32>, y: f32) -> vec3<f32> {
    return x - y * floor(x / y);
}

fn mod_v4(x: vec4<f32>, y: f32) -> vec4<f32> {
    return x - y * floor(x / y);
}

// Camera path.
fn cam_path(t: f32) -> vec3<f32> {
    let a = sin(t * 0.11);
    let b = cos(t * 0.14);
    return vec3<f32>(a * 4.0 - b * 1.5, b * 1.7 + a * 1.5, t);
}

// A fake, noisy looking field - cheaply constructed from a spherized
// sinusoidal combination.
fn map(p_in: vec3<f32>) -> f32 {
    let cam_xy = cam_path(p_in.z).xy;
    var p = vec3<f32>(p_in.x - cam_xy.x, p_in.y - cam_xy.y, p_in.z);

    let tau = 6.2831853;
    p = cos(mod_v3(p * 0.315 * 1.25 + sin(mod_v3(p.zxy * 0.875 * 1.25, tau)), tau));

    let n = length(p);

    return (n - 1.025) * 1.33;
}

// IQ-style occlusion routine.
fn cao(p: vec3<f32>, n: vec3<f32>) -> f32 {
    var sc = 1.0;
    var occ = 0.0;
    for (var i: f32 = 0.0; i < 5.0; i += 1.0) {
        let hr = 0.01 + i * 0.35 / 4.0;
        let dd = map(n * hr + p);
        occ += (hr - dd) * sc;
        sc *= 0.7;
    }
    return clamp(1.0 - occ, 0.0, 1.0);
}

// Standard normal function.
fn nr(p: vec3<f32>) -> vec3<f32> {
    let e = vec2<f32>(0.002, 0.0);
    return normalize(vec3<f32>(
        map(p + e.xyy) - map(p - e.xyy),
        map(p + e.yxy) - map(p - e.yxy),
        map(p + e.yyx) - map(p - e.yyx),
    ));
}

// Basic raymarcher.
fn trace(ro: vec3<f32>, rd: vec3<f32>) -> f32 {
    accum = 0.0;
    var t = 0.0;
    for (var i: i32 = 0; i < 128; i += 1) {
        let h = map(ro + rd * t);
        if (abs(h) < 0.001 * (t * 0.25 + 1.0) || t > FAR) {
            break;
        }
        t += h;
        if (abs(h) < 0.35) {
            accum += (0.35 - abs(h)) / 24.0;
        }
    }
    return min(t, FAR);
}

// Shadows.
fn sha(ro: vec3<f32>, rd: vec3<f32>, start: f32, end: f32, k: f32) -> f32 {
    var shade = 1.0;
    let max_iterations_shad = 24;
    var dist = start;
    for (var i: i32 = 0; i < max_iterations_shad; i += 1) {
        let h = map(ro + rd * dist);
        shade = min(shade, smoothstep(0.0, 1.0, k * h / dist));
        dist += clamp(h, 0.01, 0.2);
        if (abs(h) < 0.001 || dist > end) {
            break;
        }
    }
    return min(max(shade, 0.0) + 0.4, 1.0);
}

// Compact, self-contained version of IQ's 3D value noise function.
fn n3d(p_in: vec3<f32>) -> f32 {
    let s = vec3<f32>(7.0, 157.0, 113.0);
    let ip = floor(p_in);
    var p = p_in - ip;
    var h = vec4<f32>(0.0, s.y, s.z, s.y + s.z) + dot(ip, s);
    p = p * p * (3.0 - 2.0 * p);
    h = mix(fract(sin(mod_v4(h, 6.231589)) * 43758.5453),
            fract(sin(mod_v4(h + s.x, 6.231589)) * 43758.5453), p.x);
    let hxy = mix(h.xz, h.yw, p.y);
    return mix(hxy.x, hxy.y, p.z);
}

// Procedural substitute for the original's sampler2D texture lookup — see
// the module doc's documented-deviation note. Three octaves of `n3d`, tinted
// toward a pale fog/glass color rather than pure greyscale.
fn tex_noise(uv: vec2<f32>) -> vec3<f32> {
    let p = vec3<f32>(uv, 0.0);
    let n0 = n3d(p * 4.0);
    let n1 = n3d(p * 8.0 + vec3<f32>(17.0, 31.0, 5.0));
    let n2 = n3d(p * 16.0 + vec3<f32>(53.0, 7.0, 91.0));
    let g = n0 * 0.55 + n1 * 0.30 + n2 * 0.15;
    return g * vec3<f32>(0.75, 0.85, 1.0);
}

// Tri-Planar blending function (GPU Gems 3, Ryan Geiss), fed the procedural
// `tex_noise` above instead of a real sampler2D — see the module doc.
fn tpl(p: vec3<f32>, n_in: vec3<f32>) -> vec3<f32> {
    var n = max(abs(n_in) - 0.2, vec3<f32>(0.001));
    n /= dot(n, vec3<f32>(1.0));
    let tx = tex_noise(p.zy);
    let ty = tex_noise(p.xz);
    let tz = tex_noise(p.xy);
    return tx * tx * n.x + ty * ty * n.y + tz * tz * n.z;
}

// Simple environment mapping.
fn env_map(rd: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let col = tpl(rd * 4.0, n);
    return smoothstep(vec3<f32>(0.0), vec3<f32>(1.0), col);
}

@fragment
fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> {
    let pos = in.position.xy;
    // y-flip: FrustVsOut::position is top-left origin; shadertoy's fragCoord
    // is bottom-left (matches shader 1's convention).
    let frag_coord = vec2<f32>(pos.x, frust_u.resolution.y - pos.y);

    var u = (frag_coord - frust_u.resolution * 0.5) / frust_u.resolution.y;

    let speed = 4.0;
    let o = cam_path(frust_u.time * speed);
    let lk = cam_path(frust_u.time * speed + 0.25);
    var l = cam_path(frust_u.time * speed + 2.0) + vec3<f32>(0.0, 1.0, 0.0);

    let fov = 3.14159 / 2.0;
    let fwd = normalize(lk - o);
    let rgt = normalize(vec3<f32>(fwd.z, 0.0, -fwd.x));
    let up = cross(fwd, rgt);

    var r = fwd + fov * (u.x * rgt + u.y * up);
    r = normalize(vec3<f32>(r.xy, r.z - length(r.xy) * 0.125));

    let t = trace(o, r);

    var col = vec3<f32>(0.0);

    if (t < FAR) {
        let p = o + r * t;
        let n = nr(p);
        let svn = n;
        // Texture bump mapping dropped — see the module doc's documented
        // visual deviation. `n`/`svn` are identical here (both the
        // un-bumped normal), matching the original's `svn*.5 + n*.5` shape.

        l -= p;
        let d = max(length(l), 0.001);
        l /= d;

        let at = 1.0 / (1.0 + d * 0.05 + d * d * 0.0125);

        let ao = cao(p, n);
        let sh = sha(p, l, 0.04, d, 16.0);

        let di = max(dot(l, n), 0.0);
        let sp = pow(max(dot(reflect(r, n), l), 0.0), 64.0);
        let fr = clamp(1.0 + dot(r, n), 0.0, 1.0);

        let tx = vec3<f32>(0.05);

        col = tx * (di * 0.1 + ao * 0.25) + vec3<f32>(0.5, 0.7, 1.0) * sp * 2.0
            + vec3<f32>(1.0, 0.7, 0.4) * pow(fr, 8.0) * 0.25;

        let refl = env_map(normalize(reflect(r, svn * 0.5 + n * 0.5)), svn * 0.5 + n * 0.5);
        let refr = env_map(normalize(refract(r, svn * 0.5 + n * 0.5, 1.0 / 1.35)), svn * 0.5 + n * 0.5);

        let ref_col = mix(refr, refl, pow(fr, 5.0));

        col += ref_col * ((di * di * 0.25 + 0.75) + ao * 0.25) * 1.5;

        col = mix(col.xzy, col, di * 0.85 + 0.15);

        let acc_col = vec3<f32>(1.0, 0.3, 0.1) * accum;
        let gc = pow(min(vec3<f32>(1.5, 1.0, 1.0) * accum, vec3<f32>(1.0)), vec3<f32>(1.0, 2.5, 12.0)) * 0.5 + acc_col * 0.5;
        col += col * gc * 12.0;

        let hi = abs(mod_f(t / 1.0 + frust_u.time / 3.0, 8.0) - 8.0 / 2.0) * 2.0;
        let c_col = vec3<f32>(0.01, 0.05, 1.0) * col * (1.0 / (0.001 + hi * hi * 0.2));
        col += mix(c_col.yxz, c_col, n3d(p * 3.0));

        col *= ao * sh * at;
    }

    let fog = vec3<f32>(0.125, 0.04, 0.05) * (r.y * 0.5 + 0.5);
    col = mix(col, fog, smoothstep(0.0, 0.95, t / FAR));

    u = frag_coord / frust_u.resolution;
    col = mix(vec3<f32>(0.0), col, pow(16.0 * u.x * u.y * (1.0 - u.x) * (1.0 - u.y), 0.125) * 0.5 + 0.5);

    return vec4<f32>(sqrt(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
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
        ("Synthwave sunset", ShaderProgram::new(SYNTHWAVE_WGSL)),
        ("Glassy field", ShaderProgram::new(GLASSY_FIELD_WGSL)),
    ]
}
