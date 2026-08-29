//! The encoded-paint records the fragment shader samples, and the
//! `Rgba32Uint` texture they are serialized into.
//!
//! Each record is 16-byte aligned so it starts on a texel boundary; records
//! are written back to back, and a paint's index into the texture is the
//! texel it starts at. Sizes are part of the shader contract — a record that
//! grew or shrank would shift every paint after it — so each carries a
//! compile-time size assertion.

use bytemuck::{Pod, Zeroable};

use crate::EngineError;

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
/// Returns [`EngineError::AtlasError`] when the required height exceeds
/// `resource_texture_dim` — the point at which the reference renderer
/// asserts. There is no dedicated capacity variant for this resource yet, so
/// the general resource-allocation failure stands in.
pub fn encoded_paints_texture_height(
    texels: u32,
    resource_texture_dim: u32,
) -> Result<u32, EngineError> {
    let required = texels.div_ceil(resource_texture_dim);
    if required > resource_texture_dim {
        return Err(EngineError::AtlasError);
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
/// Returns [`EngineError::AtlasError`] on the same over-capacity condition as
/// [`encoded_paints_texture_height`].
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
