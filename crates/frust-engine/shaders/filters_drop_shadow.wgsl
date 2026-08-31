// Copyright 2026 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Derived from vello_sparse_shaders 0.2.0 (`shaders/filters/drop_shadow.wesl`),
// narrowed to the shadow-only shape `crate::filters::drop_shadow` plans for:
// no `original_texture` binding and no `composite_drop_shadow` pass, because
// this engine never composites a filter layer's unfiltered content back over
// its shadow (see that module's own doc for why).
//
// The two passes a drop shadow needs beyond a plain blur's: a shift by the
// shadow's own device-space offset, and a recolour of the blurred, offset
// alpha mask into the shadow's own premultiplied colour. Both are pure
// functions over the texture and parameters they are handed, on the same terms
// `filters_blur.wgsl`'s kernels are: no `@group`/`@binding` global, so
// prepending this file disturbs no module's derived bind-group layout.
//
// The blur passes of a shadow's own sequence need nothing from here: a
// `GpuDropShadow` block packs its header, centre weight, linear weights and
// linear offsets at the identical texel offsets a blur's own block does (see
// `crate::filters::drop_shadow::GpuDropShadow`), so `filter.wgsl`'s existing
// `PASS_BLUR_H`/`PASS_BLUR_V` cases and this prelude's own `filters_blur.wgsl`
// kernels read a shadow's blur exactly as they read a plain blur's, unmodified.

// The shadow's own device-space offset, packed in the third texel of its
// parameter block (`GpuDropShadow::dx`/`dy`).
fn get_drop_shadow_offset(texel2: vec4<u32>) -> vec2<f32> {
    return vec2<f32>(bitcast<f32>(texel2.x), bitcast<f32>(texel2.y));
}

// The shadow's own premultiplied colour, packed as RGBA8 in the third texel
// (`GpuDropShadow::color`).
fn get_drop_shadow_color(texel2: vec4<u32>) -> vec4<f32> {
    return unpack4x8unorm(texel2.z);
}

// One texel of `source_texture`, relative to `source_origin`, or transparent
// black outside `[0, source_size)` — the same terms every kernel here samples
// past the region it filters on.
//
// The check matters most on the negative side: a shadow offset can shift a
// read below zero, which an unchecked `vec2<i32>` -> `vec2<u32>` cast would
// wrap into a large, unrelated texel address rather than the transparent read
// the CPU reference gives it (`EdgeMode::None`, the mode every filter this
// engine serves is prepared with).
fn drop_shadow_load_checked(
    source_texture: texture_2d<f32>,
    source_origin: vec2<u32>,
    source_size: vec2<u32>,
    coord: vec2<f32>,
) -> vec4<f32> {
    if coord.x < 0.0 || coord.y < 0.0 || coord.x >= f32(source_size.x) || coord.y >= f32(source_size.y) {
        return vec4<f32>(0.0);
    }

    let texel = vec2<u32>(vec2<i32>(source_origin) + vec2<i32>(coord));
    return textureLoad(source_texture, texel, 0);
}

// Shift `source_texture` by the shadow's own `(dx, dy)`, rounded to the
// nearest whole texel.
//
// `floor(x + 0.5)` rather than WGSL's `round()`, which ties to even: the CPU
// reference this is compared against rounds ties away from zero, and the two
// disagree at exact `.5` offsets.
fn offset_drop_shadow(
    source_texture: texture_2d<f32>,
    source_origin: vec2<u32>,
    source_size: vec2<u32>,
    rel_coord: vec2<f32>,
    dxdy: vec2<f32>,
) -> vec4<f32> {
    let shifted = rel_coord - floor(dxdy + 0.5);
    return drop_shadow_load_checked(source_texture, source_origin, source_size, shifted);
}

// Recolour a blurred, offset alpha mask into the shadow's own premultiplied
// colour.
//
// The mask's own colour channels are discarded — a drop shadow paints the
// shape the blur produced, not the colour of the content that cast it.
// `rel_coord` is inside the source region by construction: the fragment stage
// this is called from already returned for a texel of the padding border.
fn colorize_drop_shadow(
    source_texture: texture_2d<f32>,
    source_origin: vec2<u32>,
    rel_coord: vec2<f32>,
    color: vec4<f32>,
) -> vec4<f32> {
    let texel = vec2<u32>(vec2<i32>(source_origin) + vec2<i32>(rel_coord));
    let blurred = textureLoad(source_texture, texel, 0);
    return color * blurred.a;
}
