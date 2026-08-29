// Copyright 2024 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Derived from vello_sparse_shaders 0.2.0 (`shaders/helpers/*.wesl`).
//
// Shared, binding-free helper functions: packing, quad geometry, extend
// modes, encoded-paint accessors, gradient/blur evaluation and atlas
// sampling. Every entry-point module (`strip.wgsl`, `clear.wgsl`,
// `copy.wgsl`) is compiled with this file prepended, so the WESL reference's
// `import package::helpers::...` lines are resolved by concatenation rather
// than by a resolver at build time.
//
// Two consequences of that flattening are visible below:
//
// 1. Nothing here declares a `@group`/`@binding` global. A texture a helper
//    reads is always a function parameter, so prepending this file never
//    changes an entry-point module's derived bind-group layout.
// 2. Parameters that named a texture or an extend mode in the reference are
//    renamed (`tex`, `mode`) where the reference's own module boundary was
//    the only thing keeping them from shadowing an entry-point global or a
//    function declared here.

// Mathematical constants.
const PI: f32 = 3.1415926535897932384626433832795028;
const TWO_PI: f32 = 2.0 * PI;
// Tolerance for nearly-zero comparisons. Must match `SCALAR_NEARLY_ZERO` in
// `vello_common::math`, which the CPU-side rasterizer compares against.
const NEARLY_ZERO_TOLERANCE: f32 = 1.0 / 4096.0;

// Extend modes, shared by images and gradients.
const EXTEND_PAD: u32 = 0u;
const EXTEND_REPEAT: u32 = 1u;
const EXTEND_REFLECT: u32 = 2u;

// Image rendering quality.
const IMAGE_QUALITY_LOW: u32 = 0u;
const IMAGE_QUALITY_MEDIUM: u32 = 1u;
const IMAGE_QUALITY_HIGH: u32 = 2u;

// Image source kinds.
const IMAGE_SOURCE_ATLAS: u32 = 0u;
const IMAGE_SOURCE_EXTERNAL: u32 = 1u;

// Tint modes.
const TINT_MODE_ALPHA_MASK: u32 = 0u;
const TINT_MODE_MULTIPLY: u32 = 1u;

// Gradient types.
const GRADIENT_TYPE_LINEAR: u32 = 0u;
const GRADIENT_TYPE_RADIAL: u32 = 1u;
const GRADIENT_TYPE_SWEEP: u32 = 2u;

// Radial gradient types.
const RADIAL_GRADIENT_TYPE_STANDARD: u32 = 0u;
const RADIAL_GRADIENT_TYPE_STRIP: u32 = 1u;
const RADIAL_GRADIENT_TYPE_FOCAL: u32 = 2u;

// -----------------------------------------------------------------------------
// Packing
// -----------------------------------------------------------------------------

fn unpack_u16_pair(value: u32) -> vec2<u32> {
    return vec2<u32>(value & 0xffffu, value >> 16u);
}

// -----------------------------------------------------------------------------
// Texture addressing
// -----------------------------------------------------------------------------

fn flat_index_to_texture_coord(index: u32, width: u32) -> vec2<u32> {
    return vec2<u32>(index % width, index / width);
}

// -----------------------------------------------------------------------------
// Quad geometry
// -----------------------------------------------------------------------------

fn quad_corner(vertex_index: u32) -> vec2<f32> {
    return vec2<f32>(
        f32(vertex_index & 1u),
        f32(vertex_index >> 1u),
    );
}

fn pixel_to_ndc(pixel: vec2<f32>, target_size: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        pixel.x * 2.0 / target_size.x - 1.0,
        1.0 - pixel.y * 2.0 / target_size.y,
    );
}

// -----------------------------------------------------------------------------
// Strip alpha unpacking
// -----------------------------------------------------------------------------

// Alpha textures store 16 1-byte alpha values per texel, with each color
// channel packing the 4 alpha values of a single strip column.
fn unpack_alphas_from_channel(rgba: vec4<u32>, channel_index: u32) -> u32 {
    switch channel_index {
        case 0u: { return rgba.x; }
        case 1u: { return rgba.y; }
        case 2u: { return rgba.z; }
        case 3u: { return rgba.w; }
        // Fallback, should never happen.
        default: { return rgba.x; }
    }
}

// -----------------------------------------------------------------------------
// Extend modes
// -----------------------------------------------------------------------------

fn extend_mode(t: f32, mode: u32, max: f32) -> f32 {
    switch mode {
        case EXTEND_PAD: {
            return clamp(t, 0.0, max - 1.0);
        }
        case EXTEND_REPEAT: {
            return extend_mode_normalized(t / max, mode) * max;
        }
        case EXTEND_REFLECT, default: {
            return extend_mode_normalized(t / max, mode) * max;
        }
    }
}

fn extend_mode_normalized(t: f32, mode: u32) -> f32 {
    switch mode {
        case EXTEND_PAD: {
            return clamp(t, 0.0, 1.0);
        }
        case EXTEND_REPEAT: {
            return fract(t);
        }
        case EXTEND_REFLECT, default: {
            return abs(t - 2.0 * round(0.5 * t));
        }
    }
}

// -----------------------------------------------------------------------------
// Encoded gradient accessors and evaluation
// -----------------------------------------------------------------------------

// Sample from the gradient LUT texture at the calculated position.
fn sample_gradient_lut(
    tex: texture_2d<f32>,
    t_value: f32,
    mode: u32,
    gradient_start: u32,
    texture_width: u32,
) -> vec4<f32> {
    // Apply the extend mode to t_value.
    let clamped_t = extend_mode_normalized(t_value, mode);
    // Convert t_value to a texture coordinate.
    let t_offset = u32(clamped_t * f32(texture_width - 1u));
    // Absolute position in the flat gradient texture.
    let flat_coord = gradient_start + t_offset;
    let gradient_tex_width = textureDimensions(tex).x;
    let texture_coord = flat_index_to_texture_coord(flat_coord, gradient_tex_width);
    return textureLoad(tex, texture_coord, 0);
}

// Width of the gradient's own ramp, in texels.
fn get_gradient_texture_width(texel0: vec4<u32>) -> u32 { return texel0.x & 0x0FFFFFFFu; }

// The extend mode for the gradient.
fn get_gradient_extend_mode(texel0: vec4<u32>) -> u32 { return (texel0.x >> 30u) & 3u; }

// Start coordinate in the flat gradient texture.
fn get_gradient_start(texel0: vec4<u32>) -> u32 { return texel0.y; }

// 2x2 linear part of the affine transform (columns [a,b] and [c,d]).
fn get_gradient_transform(texel0: vec4<u32>, texel1: vec4<u32>) -> mat2x2<f32> {
    return mat2x2<f32>(
        vec2<f32>(bitcast<f32>(texel0.z), bitcast<f32>(texel0.w)),
        vec2<f32>(bitcast<f32>(texel1.x), bitcast<f32>(texel1.y))
    );
}

// Translation part of the affine transform [tx, ty].
fn get_gradient_translate(texel1: vec4<u32>) -> vec2<f32> {
    return vec2<f32>(bitcast<f32>(texel1.z), bitcast<f32>(texel1.w));
}

fn apply_gradient_transform(
    texel0: vec4<u32>,
    texel1: vec4<u32>,
    fragment_pos: vec2<f32>,
) -> vec2<f32> {
    return get_gradient_transform(texel0, texel1) * fragment_pos + get_gradient_translate(texel1);
}

// Kind of radial gradient (0=Radial, 1=Strip, 2=Focal).
fn get_radial_kind(texel2: vec4<u32>) -> u32 { return texel2.x & 0x3u; }

// Whether the focal point is swapped for the radial gradient (0=false, 1=true).
fn get_radial_f_is_swapped(texel2: vec4<u32>) -> u32 { return (texel2.x >> 2u) & 1u; }

// Bias value for radial gradient calculation.
fn get_radial_bias(texel2: vec4<u32>) -> f32 { return bitcast<f32>(texel2.y); }

// Scale factor for radial gradient calculation.
fn get_radial_scale(texel2: vec4<u32>) -> f32 { return bitcast<f32>(texel2.z); }

// Focal point 0 parameter for radial gradient.
fn get_radial_fp0(texel2: vec4<u32>) -> f32 { return bitcast<f32>(texel2.w); }

// Focal point 1 parameter for radial gradient.
fn get_radial_fp1(texel3: vec4<u32>) -> f32 { return bitcast<f32>(texel3.x); }

// Focal radius 1 parameter for radial gradient.
fn get_radial_fr1(texel3: vec4<u32>) -> f32 { return bitcast<f32>(texel3.y); }

// Focal X coordinate for radial gradient.
fn get_radial_f_focal_x(texel3: vec4<u32>) -> f32 { return bitcast<f32>(texel3.z); }

// Scaled radius 0 squared parameter for the radial gradient strip kind.
fn get_radial_scaled_r0_squared(texel3: vec4<u32>) -> f32 { return bitcast<f32>(texel3.w); }

// Starting angle for sweep gradient (in radians).
fn get_sweep_start_angle(texel2: vec4<u32>) -> f32 { return bitcast<f32>(texel2.x); }

// Inverse of angle delta for sweep gradient.
fn get_sweep_inv_angle_delta(texel2: vec4<u32>) -> f32 { return bitcast<f32>(texel2.y); }

// Fast polynomial approximation for xy_to_unit_angle from Skia.
// Returns an angle in the [0, 1) range representing [0, 2*PI).
// See: https://github.com/google/skia/blob/30bba741989865c157c7a997a0caebe94921276b/src/opts/SkRasterPipeline_opts.h#L5859
fn xy_to_unit_angle(x: f32, y: f32) -> f32 {
    let xabs = abs(x);
    let yabs = abs(y);
    let slope = min(xabs, yabs) / max(xabs, yabs);
    let s = slope * slope;
    // A 7th degree polynomial approximating atan, generated with
    // sollya.gforge.inria.fr via
    // P1 = fpminimax((1/(2*Pi))*atan(x),[|1,3,5,7|],[|24...|],[2^(-40),1],relative);
    var phi = slope * (0.15912117063999176025390625 + s * (-5.185396969318389892578125e-2 + s * (2.476101927459239959716796875e-2 + s * (-7.0547382347285747528076171875e-3))));
    // Map from the first octant to the full circle using quadrant information.
    // Handle the [0, 90] degree range.
    phi = select(phi, 0.25 - phi, xabs < yabs);
    // Handle the [90, 180] degree range.
    phi = select(phi, 0.5 - phi, x < 0.0);
    // Handle the [180, 360] degree range.
    phi = select(phi, 1.0 - phi, y < 0.0);
    // Handle NaN cases (using the property that NaN != NaN).
    phi = select(phi, 0.0, phi != phi);
    return phi;
}

// Calculate a radial gradient; matches the CPU rasterizer's implementation.
// Returns [t_value, validity], where validity is 0.0 for a sample that has no
// gradient coverage at all.
fn calculate_radial_gradient(
    grad_pos: vec2<f32>,
    texel2: vec4<u32>,
    texel3: vec4<u32>,
) -> vec2<f32> {
    let x_pos = grad_pos.x;
    let y_pos = grad_pos.y;

    var t_value: f32;
    var is_valid: bool;
    let kind = get_radial_kind(texel2);

    switch kind {
        case RADIAL_GRADIENT_TYPE_STANDARD: {
            // Standard radial gradient: bias + scale * sqrt(x^2 + y^2).
            let radius = sqrt(x_pos * x_pos + y_pos * y_pos);
            t_value = get_radial_bias(texel2) + get_radial_scale(texel2) * radius;
            // Radial gradients are always valid.
            is_valid = true;
        }
        case RADIAL_GRADIENT_TYPE_STRIP: {
            // Strip gradient: x + sqrt(scaled_r0_squared - y^2).
            let p1 = get_radial_scaled_r0_squared(texel3) - y_pos * y_pos;
            // Invalid if negative under the square root.
            is_valid = p1 >= 0.0;
            if is_valid {
                t_value = x_pos + sqrt(p1);
            } else {
                // Value doesn't matter when invalid.
                t_value = 0.0;
            }
        }
        case RADIAL_GRADIENT_TYPE_FOCAL, default: {
            var t = 0.0;
            let fp0 = get_radial_fp0(texel2);
            let fp1 = get_radial_fp1(texel3);
            let fr1 = get_radial_fr1(texel3);
            let f_focal_x = get_radial_f_focal_x(texel3);
            let is_swapped = get_radial_f_is_swapped(texel2);

            // Focal flags, derived from the encoded field values.
            let is_focal_on_circle = abs(1.0 - fr1) <= NEARLY_ZERO_TOLERANCE;
            let is_well_behaved = !is_focal_on_circle && fr1 > 1.0;
            let is_natively_focal = abs(f_focal_x) <= NEARLY_ZERO_TOLERANCE;

            // Start with the valid assumption.
            is_valid = true;

            if is_focal_on_circle {
                t = x_pos + y_pos * y_pos / x_pos;
                // Check for division by zero and negative t.
                is_valid = t >= 0.0 && x_pos != 0.0;
            } else if is_well_behaved {
                t = sqrt(x_pos * x_pos + y_pos * y_pos) - x_pos * fp0;
            } else {
                // For non-well-behaved gradients, check whether the
                // calculation is valid.
                let xx = x_pos * x_pos;
                let yy = y_pos * y_pos;
                let discriminant = xx - yy;

                if is_swapped != 0u || (1.0 - f_focal_x < 0.0) {
                    t = -sqrt(discriminant) - x_pos * fp0;
                } else {
                    t = sqrt(discriminant) - x_pos * fp0;
                }

                // Invalid if the discriminant is negative or t is negative.
                is_valid = discriminant >= 0.0 && t >= 0.0;
            }

            // Apply the additional focal transforms only if still valid.
            if is_valid {
                if 1.0 - f_focal_x < 0.0 {
                    t = -t;
                }

                if !is_natively_focal {
                    t = t + fp1;
                }

                if is_swapped != 0u {
                    t = 1.0 - t;
                }
            }

            t_value = t;
        }
    }

    return vec2<f32>(t_value, select(0.0, 1.0, is_valid));
}

// -----------------------------------------------------------------------------
// Encoded blurred-rounded-rect accessors and evaluation
// -----------------------------------------------------------------------------

// 2x2 linear part of the affine transform (columns [a,b] and [c,d]).
fn get_blurred_rounded_rect_transform(texel0: vec4<u32>) -> mat2x2<f32> {
    return mat2x2<f32>(
        vec2<f32>(bitcast<f32>(texel0.x), bitcast<f32>(texel0.y)),
        vec2<f32>(bitcast<f32>(texel0.z), bitcast<f32>(texel0.w))
    );
}

// Translation part of the affine transform [tx, ty].
fn get_blurred_rounded_rect_translate(texel1: vec4<u32>) -> vec2<f32> {
    return vec2<f32>(bitcast<f32>(texel1.x), bitcast<f32>(texel1.y));
}

// Premultiplied rectangle color.
fn get_blurred_rounded_rect_color(texel1: vec4<u32>) -> vec4<f32> { return unpack4x8unorm(texel1.z); }

// Whether to paint the inverse (`1 - alpha`) of the blur coverage.
fn get_blurred_rounded_rect_invert(texel1: vec4<u32>) -> u32 { return texel1.w; }

fn get_blurred_rounded_rect_exponent(texel2: vec4<u32>) -> f32 { return bitcast<f32>(texel2.x); }

fn get_blurred_rounded_rect_recip_exponent(texel2: vec4<u32>) -> f32 { return bitcast<f32>(texel2.y); }

fn get_blurred_rounded_rect_scale(texel2: vec4<u32>) -> f32 { return bitcast<f32>(texel2.z); }

fn get_blurred_rounded_rect_std_dev_inv(texel2: vec4<u32>) -> f32 { return bitcast<f32>(texel2.w); }

fn get_blurred_rounded_rect_min_edge(texel3: vec4<u32>) -> f32 { return bitcast<f32>(texel3.x); }

fn get_blurred_rounded_rect_w(texel3: vec4<u32>) -> f32 { return bitcast<f32>(texel3.y); }

fn get_blurred_rounded_rect_h(texel3: vec4<u32>) -> f32 { return bitcast<f32>(texel3.z); }

fn get_blurred_rounded_rect_r1(texel3: vec4<u32>) -> f32 { return bitcast<f32>(texel3.w); }

fn get_blurred_rounded_rect_width(texel4: vec4<u32>) -> f32 { return bitcast<f32>(texel4.x); }

fn get_blurred_rounded_rect_height(texel4: vec4<u32>) -> f32 { return bitcast<f32>(texel4.y); }

// Approximation to erf, matching the CPU rasterizer's blur painter.
fn erf7(x: f32) -> f32 {
    let y = clamp(x * 1.1283791671, -100.0, 100.0);
    let yy = y * y;
    let z = y + (0.24295 + (0.03395 + 0.0104 * yy) * yy) * (y * yy);
    return z / sqrt(1.0 + z * z);
}

// Approximation for the convolution of a gaussian filter with a rounded
// rectangle, modelled after the CPU rasterizer's blurred-rounded-rect painter.
fn calculate_blurred_rounded_rect(
    fragment_pos: vec2<f32>,
    texel0: vec4<u32>,
    texel1: vec4<u32>,
    texel2: vec4<u32>,
    texel3: vec4<u32>,
    texel4: vec4<u32>,
) -> vec4<f32> {
    let transform = get_blurred_rounded_rect_transform(texel0);
    let translate = get_blurred_rounded_rect_translate(texel1);
    let color = get_blurred_rounded_rect_color(texel1);
    let invert = get_blurred_rounded_rect_invert(texel1);
    let exponent = get_blurred_rounded_rect_exponent(texel2);
    let recip_exponent = get_blurred_rounded_rect_recip_exponent(texel2);
    let scale = get_blurred_rounded_rect_scale(texel2);
    let std_dev_inv = get_blurred_rounded_rect_std_dev_inv(texel2);
    let min_edge = get_blurred_rounded_rect_min_edge(texel3);
    let w = get_blurred_rounded_rect_w(texel3);
    let h = get_blurred_rounded_rect_h(texel3);
    let r1 = get_blurred_rounded_rect_r1(texel3);
    let width = get_blurred_rounded_rect_width(texel4);
    let height = get_blurred_rounded_rect_height(texel4);

    let local_xy = transform * fragment_pos + translate;
    // The 0.5 and 0.0 constants correspond to the CPU painter's v1 and v0.
    let y = local_xy.y - 0.5 * height;
    let y0 = r1 + abs(y) - 0.5 * h;
    let y1 = max(y0, 0.0);

    let x = local_xy.x - 0.5 * width;
    let x0 = r1 + abs(x) - 0.5 * w;
    let x1 = max(x0, 0.0);

    let d_pos = pow(
        pow(x1, exponent) + pow(y1, exponent),
        recip_exponent,
    );
    let d_neg = min(max(x0, y0), 0.0);
    let d = d_pos + d_neg - r1;
    let blur_coverage = scale * (
        erf7(std_dev_inv * (min_edge + d)) -
        erf7(std_dev_inv * d)
    );

    // Invert alpha when the `invert` flag is set.
    let blur_alpha = select(blur_coverage, 1.0 - blur_coverage, invert != 0u);

    return color * blur_alpha;
}

// -----------------------------------------------------------------------------
// Encoded image accessors
// -----------------------------------------------------------------------------

// Encoded image layout. Must match `GpuEncodedImage` in `gpu::paint_texture`.
//
// texel0.x: image_params
//   bits 0-1: quality
//   bits 2-3: extend_x
//   bits 4-5: extend_y
//   bits 6-13: atlas_index
//   bit 14: source_kind (0=atlas, 1=external texture)
// texel0.y: image_size, packed as [width:16, height:16]
// texel0.z: image_offset, packed as [x:16, y:16]
// texel0.w/texel1.x/texel1.y/texel1.z: transform matrix [a, b, c, d]
// texel1.w/texel2.x: translation [tx, ty]
// texel2.y: premultiplied tint color packed as RGBA8 unorm
// texel2.z: tint mode
// texel2.w: transparent padding pixels around the image in the atlas

// The rendering quality of the image.
fn get_image_quality(texel0: vec4<u32>) -> u32 { return texel0.x & 0x3u; }

// The extend modes in the horizontal and vertical direction.
fn get_image_extend_modes(texel0: vec4<u32>) -> vec2<u32> {
    return vec2<u32>((texel0.x >> 2u) & 0x3u, (texel0.x >> 4u) & 0x3u);
}

// The size of the image in pixels.
fn get_image_size(texel0: vec4<u32>) -> vec2<f32> {
    return vec2<f32>(f32(texel0.y >> 16u), f32(texel0.y & 0xFFFFu));
}

// The offset of the image in pixels.
fn get_image_offset(texel0: vec4<u32>) -> vec2<f32> {
    return vec2<f32>(f32(texel0.z >> 16u), f32(texel0.z & 0xFFFFu));
}

// The atlas index containing this image.
fn get_image_atlas_index(texel0: vec4<u32>) -> u32 { return (texel0.x >> 6u) & 0xFFu; }

// Whether the image is sourced from the atlas or the externally bound texture.
fn get_image_source_kind(texel0: vec4<u32>) -> u32 { return (texel0.x >> 14u) & 0x1u; }

// 2x2 linear part of the affine transform (columns [a,b] and [c,d]).
fn get_image_transform(texel0: vec4<u32>, texel1: vec4<u32>) -> mat2x2<f32> {
    return mat2x2<f32>(
        vec2<f32>(bitcast<f32>(texel0.w), bitcast<f32>(texel1.x)),
        vec2<f32>(bitcast<f32>(texel1.y), bitcast<f32>(texel1.z))
    );
}

// Translation part of the affine transform [tx, ty].
fn get_image_translate(texel1: vec4<u32>, texel2: vec4<u32>) -> vec2<f32> {
    return vec2<f32>(bitcast<f32>(texel1.w), bitcast<f32>(texel2.x));
}

// Number of transparent padding pixels around the image in the atlas.
fn get_image_padding(texel2: vec4<u32>) -> f32 { return f32(texel2.w); }

// -----------------------------------------------------------------------------
// Atlas-array sampling
// -----------------------------------------------------------------------------

// Bilinear filtering: sample the 4 surrounding texels of the target point and
// interpolate them with a bilinear filter.
fn bilinear_sample(
    tex: texture_2d_array<f32>,
    coords: vec2<f32>,
    atlas_idx: i32,
    image_offset: vec2<f32>,
    image_size: vec2<f32>,
    _extend_modes: vec2<u32>,
    _image_padding: f32,
) -> vec4<f32> {
    let atlas_max = image_offset + image_size - vec2(1.0);
    let atlas_uv_clamped = clamp(coords, image_offset, atlas_max);
    let uv_quad = vec4(floor(atlas_uv_clamped), ceil(atlas_uv_clamped));
    let uv_frac = fract(coords);
    let a = textureLoad(tex, vec2<i32>(uv_quad.xy), atlas_idx, 0);
    let b = textureLoad(tex, vec2<i32>(uv_quad.xw), atlas_idx, 0);
    let c = textureLoad(tex, vec2<i32>(uv_quad.zy), atlas_idx, 0);
    let d = textureLoad(tex, vec2<i32>(uv_quad.zw), atlas_idx, 0);
    return mix(mix(a, b, uv_frac.y), mix(c, d, uv_frac.y), uv_frac.x);
}

// Bicubic filtering with a Mitchell filter (B=1/3, C=1/3): sample the 16
// surrounding texels of the target point and interpolate them with a cubic
// filter. The 4x4 matrix holds the coefficients of the cubic function used to
// derive the weights from the fractional part of the sample location.
fn bicubic_sample(
    tex: texture_2d_array<f32>,
    coords: vec2<f32>,
    atlas_idx: i32,
    image_offset: vec2<f32>,
    image_size: vec2<f32>,
    _extend_modes: vec2<u32>,
    _image_padding: f32,
) -> vec4<f32> {
    let atlas_max = image_offset + image_size - vec2(1.0);
    let frac_coords = fract(coords + 0.5);
    // Cubic weights for the x and y directions.
    let cx = cubic_weights(frac_coords.x);
    let cy = cubic_weights(frac_coords.y);

    // Sample the 4x4 grid around `coords`.
    let s00 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-1.5, -1.5), image_offset, atlas_max)), atlas_idx, 0);
    let s10 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-0.5, -1.5), image_offset, atlas_max)), atlas_idx, 0);
    let s20 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(0.5, -1.5), image_offset, atlas_max)), atlas_idx, 0);
    let s30 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(1.5, -1.5), image_offset, atlas_max)), atlas_idx, 0);

    let s01 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-1.5, -0.5), image_offset, atlas_max)), atlas_idx, 0);
    let s11 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-0.5, -0.5), image_offset, atlas_max)), atlas_idx, 0);
    let s21 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(0.5, -0.5), image_offset, atlas_max)), atlas_idx, 0);
    let s31 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(1.5, -0.5), image_offset, atlas_max)), atlas_idx, 0);

    let s02 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-1.5, 0.5), image_offset, atlas_max)), atlas_idx, 0);
    let s12 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-0.5, 0.5), image_offset, atlas_max)), atlas_idx, 0);
    let s22 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(0.5, 0.5), image_offset, atlas_max)), atlas_idx, 0);
    let s32 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(1.5, 0.5), image_offset, atlas_max)), atlas_idx, 0);

    let s03 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-1.5, 1.5), image_offset, atlas_max)), atlas_idx, 0);
    let s13 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-0.5, 1.5), image_offset, atlas_max)), atlas_idx, 0);
    let s23 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(0.5, 1.5), image_offset, atlas_max)), atlas_idx, 0);
    let s33 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(1.5, 1.5), image_offset, atlas_max)), atlas_idx, 0);

    // Interpolate in the x direction for each row.
    let row0 = cx.x * s00 + cx.y * s10 + cx.z * s20 + cx.w * s30;
    let row1 = cx.x * s01 + cx.y * s11 + cx.z * s21 + cx.w * s31;
    let row2 = cx.x * s02 + cx.y * s12 + cx.z * s22 + cx.w * s32;
    let row3 = cx.x * s03 + cx.y * s13 + cx.z * s23 + cx.w * s33;
    // Interpolate in the y direction.
    let result = cy.x * row0 + cy.y * row1 + cy.z * row2 + cy.w * row3;

    // Clamp alpha first, then clamp the premultiplied color channels against it.
    let a = clamp(result.a, 0.0, 1.0);
    return vec4<f32>(clamp(result.rgb, vec3(0.0), vec3(a)), a);
}

// Mitchell-Netravali cubic filter coefficients with B=1/3 and C=1/3, matching
// the CPU rasterizer's cubic resampler.
const MF: array<vec4<f32>, 4> = array<vec4<f32>, 4>(
    vec4<f32>(
        (1.0 / 6.0) / 3.0,
        -(3.0 / 6.0) / 3.0 - 1.0 / 3.0,
        (3.0 / 6.0) / 3.0 + 2.0 * 1.0 / 3.0,
        -(1.0 / 6.0) / 3.0 - 1.0 / 3.0
    ),
    vec4<f32>(
        1.0 - (2.0 / 6.0) / 3.0,
        0.0,
        -3.0 + (12.0 / 6.0) / 3.0 + 1.0 / 3.0,
        2.0 - (9.0 / 6.0) / 3.0 - 1.0 / 3.0
    ),
    vec4<f32>(
        (1.0 / 6.0) / 3.0,
        (3.0 / 6.0) / 3.0 + 1.0 / 3.0,
        3.0 - (15.0 / 6.0) / 3.0 - 2.0 * 1.0 / 3.0,
        -2.0 + (9.0 / 6.0) / 3.0 + 1.0 / 3.0
    ),
    vec4<f32>(
        0.0,
        0.0,
        -1.0 / 3.0,
        (1.0 / 6.0) / 3.0 + 1.0 / 3.0
    )
);

// The four cubic weights for a single fractional value.
fn cubic_weights(fract: f32) -> vec4<f32> {
    return vec4<f32>(
        single_weight(fract, MF[0][0], MF[0][1], MF[0][2], MF[0][3]),
        single_weight(fract, MF[1][0], MF[1][1], MF[1][2], MF[1][3]),
        single_weight(fract, MF[2][0], MF[2][1], MF[2][2], MF[2][3]),
        single_weight(fract, MF[3][0], MF[3][1], MF[3][2], MF[3][3])
    );
}

// One weight from the fractional value t and the cubic coefficients.
fn single_weight(t: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    return t * (t * (t * d + c) + b) + a;
}

// -----------------------------------------------------------------------------
// External-texture sampling
// -----------------------------------------------------------------------------

// The atlas-array samplers above, restated for a plain 2D texture: an
// externally bound image is a whole texture rather than a page of the array.

fn external_bilinear_sample(
    tex: texture_2d<f32>,
    coords: vec2<f32>,
    image_offset: vec2<f32>,
    image_size: vec2<f32>,
) -> vec4<f32> {
    let image_max = image_offset + image_size - vec2(1.0);
    let clamped_coords = clamp(coords, image_offset, image_max);
    let coord_quad = vec4(floor(clamped_coords), ceil(clamped_coords));
    let coord_frac = fract(coords);
    let a = textureLoad(tex, vec2<i32>(coord_quad.xy), 0);
    let b = textureLoad(tex, vec2<i32>(coord_quad.xw), 0);
    let c = textureLoad(tex, vec2<i32>(coord_quad.zy), 0);
    let d = textureLoad(tex, vec2<i32>(coord_quad.zw), 0);
    return mix(mix(a, b, coord_frac.y), mix(c, d, coord_frac.y), coord_frac.x);
}

fn external_bicubic_sample(
    tex: texture_2d<f32>,
    coords: vec2<f32>,
    image_offset: vec2<f32>,
    image_size: vec2<f32>,
) -> vec4<f32> {
    let image_max = image_offset + image_size - vec2(1.0);
    let frac_coords = fract(coords + 0.5);
    let cx = cubic_weights(frac_coords.x);
    let cy = cubic_weights(frac_coords.y);

    let s00 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-1.5, -1.5), image_offset, image_max)), 0);
    let s10 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-0.5, -1.5), image_offset, image_max)), 0);
    let s20 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(0.5, -1.5), image_offset, image_max)), 0);
    let s30 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(1.5, -1.5), image_offset, image_max)), 0);

    let s01 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-1.5, -0.5), image_offset, image_max)), 0);
    let s11 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-0.5, -0.5), image_offset, image_max)), 0);
    let s21 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(0.5, -0.5), image_offset, image_max)), 0);
    let s31 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(1.5, -0.5), image_offset, image_max)), 0);

    let s02 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-1.5, 0.5), image_offset, image_max)), 0);
    let s12 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-0.5, 0.5), image_offset, image_max)), 0);
    let s22 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(0.5, 0.5), image_offset, image_max)), 0);
    let s32 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(1.5, 0.5), image_offset, image_max)), 0);

    let s03 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-1.5, 1.5), image_offset, image_max)), 0);
    let s13 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(-0.5, 1.5), image_offset, image_max)), 0);
    let s23 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(0.5, 1.5), image_offset, image_max)), 0);
    let s33 = textureLoad(tex, vec2<i32>(clamp(coords + vec2(1.5, 1.5), image_offset, image_max)), 0);

    let row0 = cx.x * s00 + cx.y * s10 + cx.z * s20 + cx.w * s30;
    let row1 = cx.x * s01 + cx.y * s11 + cx.z * s21 + cx.w * s31;
    let row2 = cx.x * s02 + cx.y * s12 + cx.z * s22 + cx.w * s32;
    let row3 = cx.x * s03 + cx.y * s13 + cx.z * s23 + cx.w * s33;
    let result = cy.x * row0 + cy.y * row1 + cy.z * row2 + cy.w * row3;

    // Clamp alpha first, then clamp the premultiplied color channels against it.
    let a = clamp(result.a, 0.0, 1.0);
    return vec4<f32>(clamp(result.rgb, vec3(0.0), vec3(a)), a);
}

fn sample_external_image(
    tex: texture_2d<f32>,
    quality: u32,
    coords: vec2<f32>,
    image_offset: vec2<f32>,
    image_size: vec2<f32>,
) -> vec4<f32> {
    if quality == IMAGE_QUALITY_HIGH {
        return external_bicubic_sample(tex, coords, image_offset, image_size);
    }
    if quality == IMAGE_QUALITY_MEDIUM {
        return external_bilinear_sample(
            tex,
            coords - vec2(0.5),
            image_offset,
            image_size,
        );
    }
    return textureLoad(tex, vec2<u32>(coords), 0);
}
