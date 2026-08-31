// Copyright 2026 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Derived from vello_sparse_shaders 0.2.0 (`shaders/filter.wesl`, the
// `downscale`/`upscale`/`convolve` half the reference splits out as
// `filters/scale.wesl` and `filters/blur.wesl`). Its
// `import package::helpers::...` lines are resolved by prepending
// `helpers.wgsl` at load time (`crate::filters::FILTER`).
//
// The Gaussian-blur kernels: the two rescaling steps a decimated blur is built
// from, and the separable convolution run between them. Each one is a pure
// function over the textures it is handed.
//
// Like `helpers.wgsl`, this file declares no `@group`/`@binding` global — a
// kernel that reads a texture takes the texture and the sampler as parameters,
// so prepending it to `filter.wgsl` cannot disturb that module's derived
// bind-group layout.
//
// All three kernels sample through GPU-native bilinear filtering rather than
// averaging texel loads, which is what lets a decimation step cost four samples
// instead of sixteen and a convolution tap cost one sample instead of two.
// Every one of those samples goes through `sample_region_bilinear`, which is
// what holds the kernels' one invariant: a tap outside the region being
// filtered reads transparent black, whatever the sampler's address mode would
// have given it there.
//
// `textureSampleLevel` rather than `textureSample` throughout: the convolution
// loop runs a dynamic number of iterations, and an implicit-derivative sample
// inside non-uniform control flow is not accepted by the Direct3D backend.

// One bilinear sample of the source region, with every tap outside the region
// reading transparent black.
//
// `rel` is the sample position in texels, relative to the region's own origin,
// at texel centres; the region spans `[0, source_size)` from there.
//
// The kernels cannot leave this to the sampler. The far side of a region is
// already transparent — a filter round clears its whole destination page, and
// a page is sized a padding border wider than the region it holds — but the
// *near* side is not: a filter layer's region sits at its page's own origin,
// so a tap below zero leaves the texture entirely and `ClampToEdge` answers it
// with the region's own edge texel. That is the region's real content whenever
// the layer's expanded bounds were cut off at device zero (a full-screen
// backdrop blur), and replicating it there paints a smear the CPU rasterizer's
// `EdgeMode::None` never produces.
//
// Both halves are exact rather than approximate, which is what lets the same
// helper serve a tap that straddles the boundary as well as one wholly outside
// it. Clamping the sample to the region's last texel centre turns a straddling
// tap into a plain texel read of the one texel of the pair that is inside the
// region; the per-axis coverage below is that texel's own bilinear weight, so
// the product is what the sample would have been had the outside texel held
// transparent black. Inside the region both factors are exactly one and the
// clamp is a no-op, so an interior tap is bit-for-bit the sample it always was.
fn sample_region_bilinear(
    source_texture: texture_2d<f32>,
    linear_sampler: sampler,
    source_origin: vec2<u32>,
    source_size: vec2<u32>,
    rel: vec2<f32>,
) -> vec4<f32> {
    let extent = vec2<f32>(source_size);
    // `max(.., 0)` for the degenerate empty region, whose last texel centre
    // would otherwise sit before its first; the coverage below is zero there
    // anyway, so the clamp only has to stay in range.
    let last = max(extent - vec2(1.0), vec2(0.0));
    let clamped = clamp(rel, vec2(0.0), last);
    let coverage = clamp(rel + vec2(1.0), vec2(0.0), vec2(1.0))
        * clamp(extent - rel, vec2(0.0), vec2(1.0));

    let source_texel = vec2<f32>(source_origin) + clamped;
    let source_texture_size = vec2<f32>(textureDimensions(source_texture));
    let sampled = textureSampleLevel(source_texture, linear_sampler, (source_texel + 0.5) / source_texture_size, 0.0);

    return sampled * (coverage.x * coverage.y);
}

// The most linear-sampling tap pairs per side a kernel can carry.
//
// Keep in sync with `MAX_TAPS_PER_SIDE` in `crate::filters::blur`: the
// `vec3<f32>` weight and offset accessors below assume exactly this many.
const MAX_TAPS_PER_SIDE: u32 = 3u;

// Halve one axis pair of the source, writing the texel of `dest_origin`'s
// region that `position` names.
//
// This follows the CPU rasterizer, which downscales by applying a [1,3,3,1]/8
// binomial filter along each axis. On the GPU the two axes collapse into one
// pass: four bilinear samples placed a quarter-texel outside each pair of
// source texels reproduce the same [1,3,3,1] weighting in both directions, so
// sixteen texel reads become four samples.
fn filter_downscale(
    source_texture: texture_2d<f32>,
    linear_sampler: sampler,
    position: vec4<f32>,
    source_origin: vec2<u32>,
    source_size: vec2<u32>,
    dest_origin: vec2<u32>,
) -> vec4<f32> {
    let frag_coord = vec2<u32>(position.xy);
    let rel = vec2<i32>(frag_coord - dest_origin);
    let source_rel = vec2<f32>(rel * 2);

    // The four sample points are [src - 1, src, src + 1, src + 2]. To weight
    // them [1,3,3,1], the low sample shifts 0.25 texels towards the low side
    // and the high sample 1.25 towards the high side; bilinear filtering then
    // performs the 1:3 blend of each pair for free.
    let lo = vec2<f32>(-0.25);
    let hi = vec2<f32>(1.25);

    let s00 = sample_region_bilinear(source_texture, linear_sampler, source_origin, source_size, source_rel + vec2(lo.x, lo.y));
    let s01 = sample_region_bilinear(source_texture, linear_sampler, source_origin, source_size, source_rel + vec2(lo.x, hi.y));
    let s10 = sample_region_bilinear(source_texture, linear_sampler, source_origin, source_size, source_rel + vec2(hi.x, lo.y));
    let s11 = sample_region_bilinear(source_texture, linear_sampler, source_origin, source_size, source_rel + vec2(hi.x, hi.y));

    return (s00 + s01 + s10 + s11) * 0.25;
}

// Double one axis pair of the source, writing the texel of `dest_origin`'s
// region that `position` names.
//
// The reconstruction the downscale above is paired with: each output texel is
// 75% of the decimated texel covering it and 25% of the neighbour on the side
// it sits, which one bilinear sample placed a quarter-texel off centre gives
// directly.
fn filter_upscale(
    source_texture: texture_2d<f32>,
    linear_sampler: sampler,
    position: vec4<f32>,
    source_origin: vec2<u32>,
    source_size: vec2<u32>,
    dest_origin: vec2<u32>,
) -> vec4<f32> {
    let frag_coord = vec2<u32>(position.xy);
    let rel = vec2<i32>(frag_coord - dest_origin);
    let source_base = vec2<f32>(rel / 2);
    let phase = vec2<f32>(rel % 2);

    // Even phase leans towards the low neighbour, odd phase towards the high
    // one.
    let sample_offset = select(vec2(-0.25), vec2(0.25), phase == vec2(1.0));

    return sample_region_bilinear(source_texture, linear_sampler, source_origin, source_size, source_base + sample_offset);
}

// One separable Gaussian pass along `dir` (`(1, 0)` horizontal, `(0, 1)`
// vertical), reading the source texel `source_rel` names inside the source
// region at `source_origin`.
//
// The discrete kernel the CPU rasterizer convolves with is pre-merged on the
// host into a centre weight plus up to `MAX_TAPS_PER_SIDE` bilinear tap pairs
// (see `crate::filters::blur::LinearKernel`): each pair of adjacent kernel taps
// becomes one sample at a fractional offset between them, weighted by their
// sum. The result is the same Gaussian with roughly half the samples.
fn filter_convolve(
    source_texture: texture_2d<f32>,
    linear_sampler: sampler,
    source_origin: vec2<u32>,
    source_size: vec2<u32>,
    source_rel: vec2<f32>,
    dir: vec2<f32>,
    n_linear_taps: u32,
    center_weight: f32,
    weights: vec3<f32>,
    offsets: vec3<f32>,
) -> vec4<f32> {
    var color = sample_region_bilinear(source_texture, linear_sampler, source_origin, source_size, source_rel) * center_weight;

    // Indexing a `vec3` dynamically is not expressible, so the merged weights
    // and offsets are copied into arrays the loop can index.
    var weights_arr: array<f32, 3>;
    weights_arr[0] = weights.x;
    weights_arr[1] = weights.y;
    weights_arr[2] = weights.z;

    var offsets_arr: array<f32, 3>;
    offsets_arr[0] = offsets.x;
    offsets_arr[1] = offsets.y;
    offsets_arr[2] = offsets.z;

    // The kernel is symmetric, so each tap contributes on both sides of the
    // centre at the same weight.
    for (var i = 0u; i < n_linear_taps; i++) {
        let w = weights_arr[i];
        let d = dir * offsets_arr[i];
        color += sample_region_bilinear(source_texture, linear_sampler, source_origin, source_size, source_rel + d) * w;
        color += sample_region_bilinear(source_texture, linear_sampler, source_origin, source_size, source_rel - d) * w;
    }

    return color;
}
