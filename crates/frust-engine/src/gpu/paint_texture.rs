//! The encoded-paint records the fragment shader samples, and the
//! `Rgba32Uint` texture they are serialized into.
//!
//! Each record is 16-byte aligned so it starts on a texel boundary; records
//! are written back to back, and a paint's index into the texture is the
//! texel it starts at. Sizes are part of the shader contract — a record that
//! grew or shrank would shift every paint after it — so each carries a
//! compile-time size assertion.

use bytemuck::{Pod, Zeroable};

use vello_common::encode::{
    EncodedBlurredRoundedRectangle, EncodedGradient, EncodedKind, EncodedPaint, RadialKind,
};

use crate::EngineError;
use crate::cache::CachedRamp;

use super::strips::PaintType;
use super::{MIN_RESOURCE_TEXTURE_HEIGHT, TEXEL_BYTES_SHIFT, resource_texture_descriptor};

/// An encoded image paint.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuEncodedImage {
    /// Packed sampling quality, extend modes and atlas index:
    /// bits 6-13 `atlas_index`, bits 4-5 `extend_y`, bits 2-3 `extend_x`,
    /// bits 0-1 `quality`.
    pub image_params: u32,
    /// Packed image width and height.
    pub image_size: u32,
    /// Offset of the image within the atlas texture, in pixels.
    pub image_offset: u32,
    /// Transform matrix `[a, b, c, d, tx, ty]`.
    pub transform: [f32; 6],
    /// Premultiplied tint color, packed as RGBA8 unorm.
    pub tint: u32,
    /// Tint mode.
    pub tint_mode: u32,
    /// Transparent padding pixels around the image in the atlas.
    pub image_padding: u32,
}

/// An encoded linear gradient paint.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuLinearGradient {
    /// Packed gradient-texture width and extend mode
    /// ([`pack_texture_width_and_extend_mode`]).
    pub texture_width_and_extend_mode: u32,
    /// Start coordinate in the flat gradient texture.
    pub gradient_start: u32,
    /// Transform matrix `[a, b, c, d, tx, ty]`.
    pub transform: [f32; 6],
}

/// An encoded radial gradient paint.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuRadialGradient {
    /// Packed gradient-texture width and extend mode
    /// ([`pack_texture_width_and_extend_mode`]).
    pub texture_width_and_extend_mode: u32,
    /// Start coordinate in the flat gradient texture.
    pub gradient_start: u32,
    /// Transform matrix `[a, b, c, d, tx, ty]`.
    pub transform: [f32; 6],
    /// Packed gradient kind and focal-swap flag
    /// ([`pack_kind_and_f_is_swapped`]).
    pub kind_and_f_is_swapped: u32,
    /// Bias term.
    pub bias: f32,
    /// Scale factor.
    pub scale: f32,
    /// First focal-point parameter.
    pub fp0: f32,
    /// Second focal-point parameter.
    pub fp1: f32,
    /// Focal radius parameter.
    pub fr1: f32,
    /// Focal x coordinate.
    pub f_focal_x: f32,
    /// Scaled inner radius, squared (strip kind).
    pub scaled_r0_squared: f32,
}

/// An encoded sweep gradient paint.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuSweepGradient {
    /// Packed gradient-texture width and extend mode
    /// ([`pack_texture_width_and_extend_mode`]).
    pub texture_width_and_extend_mode: u32,
    /// Start coordinate in the flat gradient texture.
    pub gradient_start: u32,
    /// Transform matrix `[a, b, c, d, tx, ty]`.
    pub transform: [f32; 6],
    /// Angle the sweep starts at, in radians.
    pub start_angle: f32,
    /// Reciprocal of the sweep's angle delta.
    pub inv_angle_delta: f32,
    /// Padding to the 16-byte record alignment.
    pub _padding: [u32; 2],
}

/// An encoded blurred rounded rectangle paint.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuBlurredRoundedRect {
    /// Transform matrix `[a, b, c, d, tx, ty]`.
    pub transform: [f32; 6],
    /// Premultiplied color, packed as RGBA8 unorm.
    pub color: u32,
    /// Whether to paint the inverse (`1 - alpha`) of the blur coverage.
    pub invert: u32,
    /// Blur parameters: exponent, reciprocal exponent, scale, inverse
    /// standard deviation.
    pub params0: [f32; 4],
    /// Blur parameters: minimum edge length, adjusted width, adjusted height,
    /// outer radius.
    pub params1: [f32; 4],
    /// Rectangle size `[width, height]`.
    pub size: [f32; 2],
    /// Padding to the 16-byte record alignment.
    pub _padding1: [u32; 2],
}

const _: () = assert!(size_of::<GpuEncodedImage>() == 48);
const _: () = assert!(size_of::<GpuLinearGradient>() == 32);
const _: () = assert!(size_of::<GpuRadialGradient>() == 64);
const _: () = assert!(size_of::<GpuSweepGradient>() == 48);
const _: () = assert!(size_of::<GpuBlurredRoundedRect>() == 80);

/// One encoded paint of any kind.
///
/// Records are compared by their bytes ([`GpuEncodedPaint::as_bytes`]) rather
/// than field-wise: the bytes are the contract the shader reads.
#[derive(Debug, Clone, Copy)]
pub enum GpuEncodedPaint {
    /// An image.
    Image(GpuEncodedImage),
    /// A linear gradient.
    LinearGradient(GpuLinearGradient),
    /// A radial gradient.
    RadialGradient(GpuRadialGradient),
    /// A sweep gradient.
    SweepGradient(GpuSweepGradient),
    /// A blurred rounded rectangle.
    BlurredRoundedRect(GpuBlurredRoundedRect),
}

macro_rules! paint_record_bytes {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl $ty {
                /// This record's bytes, exactly as they land in the
                /// encoded-paint texture.
                #[must_use]
                pub fn as_bytes(&self) -> &[u8] {
                    bytemuck::bytes_of(self)
                }
            }
        )+
    };
}

paint_record_bytes!(
    GpuEncodedImage,
    GpuLinearGradient,
    GpuRadialGradient,
    GpuSweepGradient,
    GpuBlurredRoundedRect,
);

impl GpuEncodedPaint {
    /// The paint type a strip instance names this record by.
    #[must_use]
    pub const fn paint_type(&self) -> PaintType {
        match self {
            Self::Image(_) => PaintType::Image,
            Self::LinearGradient(_) => PaintType::LinearGradient,
            Self::RadialGradient(_) => PaintType::RadialGradient,
            Self::SweepGradient(_) => PaintType::SweepGradient,
            Self::BlurredRoundedRect(_) => PaintType::BlurredRoundedRect,
        }
    }

    /// This paint's bytes, exactly as they land in the encoded-paint texture.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Image(paint) => paint.as_bytes(),
            Self::LinearGradient(paint) => paint.as_bytes(),
            Self::RadialGradient(paint) => paint.as_bytes(),
            Self::SweepGradient(paint) => paint.as_bytes(),
            Self::BlurredRoundedRect(paint) => paint.as_bytes(),
        }
    }

    /// This paint's size in bytes — always a multiple of the 16-byte texel.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.as_bytes().len()
    }

    /// The number of texels this paint occupies.
    #[must_use]
    pub fn texel_len(&self) -> u32 {
        (self.byte_len() as u32) >> TEXEL_BYTES_SHIFT
    }

    /// The total byte length of `paints` serialized back to back.
    #[must_use]
    pub fn serialized_len(paints: &[Self]) -> usize {
        paints.iter().map(Self::byte_len).sum()
    }

    /// Serializes `paints` back to back into the front of `buffer`, returning
    /// the number of bytes written.
    ///
    /// `buffer` is the full padded upload buffer, so anything past the
    /// returned length keeps whatever it already held — a paint index only
    /// ever addresses a record that was written.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::AtlasError`] when `buffer` is shorter than the
    /// serialized paints, rather than writing a partial record.
    pub fn serialize_to_buffer(paints: &[Self], buffer: &mut [u8]) -> Result<usize, EngineError> {
        let required = Self::serialized_len(paints);
        if buffer.len() < required {
            return Err(EngineError::AtlasError);
        }
        let mut offset = 0;
        for paint in paints {
            let bytes = paint.as_bytes();
            buffer[offset..offset + bytes.len()].copy_from_slice(bytes);
            offset += bytes.len();
        }
        Ok(offset)
    }

    /// The texel each paint in `paints` starts at, in serialization order —
    /// the index the shader looks a paint up by.
    #[must_use]
    pub fn texel_offsets(paints: &[Self]) -> Vec<u32> {
        let mut offset = 0;
        paints
            .iter()
            .map(|paint| {
                let start = offset;
                offset += paint.texel_len();
                start
            })
            .collect()
    }
}

/// Lower one `vello_common` encoded paint into the record the strip shader
/// samples, or `None` when the engine cannot resolve it yet.
///
/// `ramp` is where the paint's colour ramp was made resident, which only a
/// gradient needs and only a caller that already serviced the frame's LUT
/// requests can supply — a gradient reaching here without one is dropped
/// rather than pointed at whatever texels the LUT texture happens to hold.
///
/// A blurred rounded rectangle carries everything its record needs inside the
/// encoded entry itself (see [`lower_blurred_rounded_rect`]), so it always
/// lowers.
///
/// An image paint answers `None` here rather than taking a third parameter:
/// its record needs the atlas rectangle the image's residency was allocated
/// at, which — unlike a ramp's cache-key lookup — is a stateful, per-frame
/// resolution keyed by an opaque `vello_common::paint::ImageId` (see
/// [`crate::renderer`]'s image registry) rather than a value this function's
/// signature can carry without breaking every other caller of it.
/// [`crate::gpu::atlas::lower_encoded_image`] is the image counterpart this
/// dispatcher intentionally does not fold in; the renderer calls it directly
/// once a frame's residency is known. An external texture answers `None`
/// unconditionally — the engine does not bind one yet (Phase 9).
#[must_use]
pub fn lower_encoded_paint(
    paint: &EncodedPaint,
    ramp: Option<CachedRamp>,
) -> Option<GpuEncodedPaint> {
    match paint {
        EncodedPaint::Gradient(gradient) => Some(lower_gradient(gradient, ramp?)),
        EncodedPaint::BlurredRoundedRect(entry) => Some(lower_blurred_rounded_rect(entry)),
        EncodedPaint::Image(_) | EncodedPaint::ExternalTexture(_) => None,
    }
}

/// Lower a blurred rounded rectangle's encoded entry into the record the
/// strip shader's `calculate_blurred_rounded_rect` reads.
///
/// `entry.transform` is already the full inverse affine (device space into
/// the rectangle's own local, origin-zeroed space) — the same
/// "already-inverted, narrow to `f32`" shape [`lower_gradient`] applies to its
/// own transform — and `entry.x_advance`/`entry.y_advance` are redundant with
/// that transform's own linear part, so neither is read here.
fn lower_blurred_rounded_rect(entry: &EncodedBlurredRoundedRectangle) -> GpuEncodedPaint {
    let transform = entry.transform.as_coeffs().map(|coeff| coeff as f32);

    GpuEncodedPaint::BlurredRoundedRect(GpuBlurredRoundedRect {
        transform,
        color: entry.color.as_premul_rgba8().to_u32(),
        invert: u32::from(entry.invert),
        params0: [
            entry.exponent,
            entry.recip_exponent,
            entry.scale,
            entry.std_dev_inv,
        ],
        params1: [entry.min_edge, entry.w, entry.h, entry.r1],
        size: [entry.width, entry.height],
        _padding1: [0, 0],
    })
}

/// Lower a gradient whose ramp is resident at `ramp`.
///
/// The transform is the gradient's own encoded transform — already the inverse
/// mapping from device space into gradient space — narrowed to `f32` because
/// that is the width the record and the shader both read it at.
fn lower_gradient(gradient: &EncodedGradient, ramp: CachedRamp) -> GpuEncodedPaint {
    let transform = gradient.transform.as_coeffs().map(|coeff| coeff as f32);
    let texture_width_and_extend_mode =
        pack_texture_width_and_extend_mode(ramp.width, extend_mode(gradient.extend));
    let gradient_start = ramp.lut_start;

    match &gradient.kind {
        EncodedKind::Linear(_) => GpuEncodedPaint::LinearGradient(GpuLinearGradient {
            texture_width_and_extend_mode,
            gradient_start,
            transform,
        }),
        EncodedKind::Radial(radial) => {
            let shape = RadialShape::of(radial);
            GpuEncodedPaint::RadialGradient(GpuRadialGradient {
                texture_width_and_extend_mode,
                gradient_start,
                transform,
                kind_and_f_is_swapped: pack_kind_and_f_is_swapped(shape.kind, shape.f_is_swapped),
                bias: shape.bias,
                scale: shape.scale,
                fp0: shape.fp0,
                fp1: shape.fp1,
                fr1: shape.fr1,
                f_focal_x: shape.f_focal_x,
                scaled_r0_squared: shape.scaled_r0_squared,
            })
        }
        EncodedKind::Sweep(sweep) => GpuEncodedPaint::SweepGradient(GpuSweepGradient {
            texture_width_and_extend_mode,
            gradient_start,
            transform,
            start_angle: sweep.start_angle,
            inv_angle_delta: sweep.inv_angle_delta,
            _padding: [0, 0],
        }),
    }
}

/// The shader's extend-mode numbering.
const fn extend_mode(extend: peniko::Extend) -> u32 {
    match extend {
        peniko::Extend::Pad => 0,
        peniko::Extend::Repeat => 1,
        peniko::Extend::Reflect => 2,
    }
}

/// The radial gradient parameters flattened into the one field set the record
/// carries.
///
/// The three radial shapes populate disjoint subsets of those fields, and the
/// shader selects between them on `kind`; collecting them here keeps the
/// record's construction one struct literal rather than a match arm per field.
struct RadialShape {
    kind: u32,
    bias: f32,
    scale: f32,
    fp0: f32,
    fp1: f32,
    fr1: f32,
    f_focal_x: f32,
    f_is_swapped: bool,
    scaled_r0_squared: f32,
}

impl RadialShape {
    /// The unused fields of every shape, which the shader does not read for
    /// that `kind`.
    const ZERO: Self = Self {
        kind: 0,
        bias: 0.0,
        scale: 0.0,
        fp0: 0.0,
        fp1: 0.0,
        fr1: 0.0,
        f_focal_x: 0.0,
        f_is_swapped: false,
        scaled_r0_squared: 0.0,
    };

    fn of(radial: &RadialKind) -> Self {
        match radial {
            RadialKind::Radial { bias, scale } => Self {
                kind: 0,
                bias: *bias,
                scale: *scale,
                ..Self::ZERO
            },
            RadialKind::Strip { scaled_r0_squared } => Self {
                kind: 1,
                scaled_r0_squared: *scaled_r0_squared,
                ..Self::ZERO
            },
            RadialKind::Focal {
                focal_data,
                fp0,
                fp1,
            } => Self {
                kind: 2,
                // The focal shape reuses `bias`/`scale` as a second copy of the
                // focal pair, matching the reference encoding the shader reads.
                bias: *fp0,
                scale: *fp1,
                fp0: *fp0,
                fp1: *fp1,
                fr1: focal_data.fr1,
                f_focal_x: focal_data.f_focal_x,
                f_is_swapped: focal_data.f_is_swapped,
                scaled_r0_squared: 0.0,
            },
        }
    }
}

/// The descriptor for an encoded-paint texture of `width` x `height` texels.
#[must_use]
pub fn encoded_paints_texture_descriptor(
    width: u32,
    height: u32,
) -> wgpu::TextureDescriptor<'static> {
    resource_texture_descriptor("frust-engine encoded paints texture", width, height)
}

/// The encoded-paint texture height needed to hold `texels` records' worth of
/// texels in a texture `resource_texture_dim` texels wide.
///
/// Never drops below [`MIN_RESOURCE_TEXTURE_HEIGHT`], since a texture of
/// height zero cannot be created.
///
/// # Errors
///
/// Returns [`EngineError::PaintCapacity`] when the required height exceeds
/// `resource_texture_dim` — the point at which the reference renderer
/// asserts. Paint records past that ceiling have nowhere to live, and a frame
/// path reports it rather than panicking.
pub fn encoded_paints_texture_height(
    texels: u32,
    resource_texture_dim: u32,
) -> Result<u32, EngineError> {
    let required = texels.div_ceil(resource_texture_dim);
    if required > resource_texture_dim {
        return Err(EngineError::PaintCapacity);
    }
    Ok(required.max(MIN_RESOURCE_TEXTURE_HEIGHT))
}

/// The height an encoded-paint texture currently `current_height` tall must
/// be recreated at to hold `texels` texels, or `None` when the existing
/// texture already fits.
///
/// The texture grows only, like the alpha texture
/// ([`super::grow_alpha_texture_height`]).
///
/// # Errors
///
/// Returns [`EngineError::PaintCapacity`] on the same over-capacity condition
/// as [`encoded_paints_texture_height`].
pub fn grow_encoded_paints_texture_height(
    current_height: u32,
    texels: u32,
    resource_texture_dim: u32,
) -> Result<Option<u32>, EngineError> {
    let required = encoded_paints_texture_height(texels, resource_texture_dim)?;
    Ok((required > current_height).then_some(required))
}

/// The bit of a packed texture-width-and-extend-mode value the extend mode
/// starts at.
const EXTEND_MODE_SHIFT: u32 = 30;

/// The mask covering the texture width in a packed
/// texture-width-and-extend-mode value.
const TEXTURE_WIDTH_MASK: u32 = (1 << EXTEND_MODE_SHIFT) - 1;

/// Packs a gradient texture's width (bits 0-29) and extend mode (bits 30-31,
/// `0` pad / `1` repeat / `2` reflect) into one word.
///
/// A width past [`TEXTURE_WIDTH_MASK`] or an extend mode past `2` is masked
/// rather than reported: both are produced by this crate's own encoding, not
/// by caller input, and the precondition is checked in debug builds.
#[must_use]
pub fn pack_texture_width_and_extend_mode(texture_width: u32, extend_mode: u32) -> u32 {
    debug_assert!(extend_mode <= 2, "extend mode must be 0, 1 or 2");
    debug_assert!(
        texture_width <= TEXTURE_WIDTH_MASK,
        "gradient texture width {texture_width} exceeds {TEXTURE_WIDTH_MASK}",
    );
    (extend_mode << EXTEND_MODE_SHIFT) | (texture_width & TEXTURE_WIDTH_MASK)
}

/// The texture width and extend mode packed by
/// [`pack_texture_width_and_extend_mode`].
#[must_use]
pub const fn unpack_texture_width_and_extend_mode(packed: u32) -> (u32, u32) {
    (packed & TEXTURE_WIDTH_MASK, packed >> EXTEND_MODE_SHIFT)
}

/// Packs a radial gradient's kind (bits 0-1, `0` radial / `1` strip / `2`
/// focal) and focal-swap flag (bit 2) into one word.
#[must_use]
pub fn pack_kind_and_f_is_swapped(kind: u32, f_is_swapped: bool) -> u32 {
    debug_assert!(kind <= 2, "radial gradient kind must be 0, 1 or 2");
    (kind & 0b11) | (u32::from(f_is_swapped) << 2)
}
