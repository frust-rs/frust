//! GPU-side data layouts the strip shaders read.
//!
//! Every type in this module is byte-compatible with the sparse-strip
//! reference renderer's own layouts, so the ported WGSL reads them unchanged.
//! Layout drift is a silent mis-render rather than a compile error, so each
//! struct carries a compile-time size assertion beside it and the integration
//! tests pin the exact bytes.
//!
//! Nothing here touches a `wgpu::Device`: sizing, addressing and packing are
//! pure functions over plain values, host-testable with no GPU in the loop —
//! the same pure-decision split `frust-gpu` follows. The one live-GPU input
//! these functions need is `frust_gpu::TierCaps::resource_texture_dim`, which
//! the caller passes in as a plain `u32`.
//!
//! Two resource textures are described here, both `Rgba32Uint`:
//!
//! 1. the alpha texture, holding the strip renderer's 1-byte coverage values
//!    16 to a texel (4 per channel), and
//! 2. the encoded-paint texture ([`paint_texture`]), holding 16-byte-aligned
//!    paint records back to back.
//!
//! Both are sized `resource_texture_dim` texels wide so the shader can turn a
//! flat index into a texel coordinate with a shift, and both grow in height
//! only — never shrink — up to the same dimension, past which the frame path
//! returns an error instead of asserting.

pub mod config;
pub mod paint_texture;
pub mod pipelines;
pub mod shader_src;
pub mod strips;

pub use config::{GpuConfig, tex_width_bits};
pub use paint_texture::{
    GpuBlurredRoundedRect, GpuEncodedImage, GpuEncodedPaint, GpuLinearGradient, GpuRadialGradient,
    GpuSweepGradient,
};
pub use pipelines::{EnginePipeline, EngineShaderModule, EngineShaders};
pub use strips::{GpuStrip, StripDraw};

use crate::EngineError;

/// The texture format both resource textures (alphas, encoded paints) use.
///
/// `Rgba32Uint` gives 16 bytes per texel with no format conversion applied on
/// the sampling side, which is what lets the same texel hold either 16 packed
/// coverage bytes or one quarter of a paint record.
pub const RESOURCE_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;

/// The usages both resource textures are created with: sampled by the strip
/// shaders, written by a queue upload, never a render attachment.
pub const RESOURCE_TEXTURE_USAGES: wgpu::TextureUsages =
    wgpu::TextureUsages::TEXTURE_BINDING.union(wgpu::TextureUsages::COPY_DST);

/// `log2` of [`TEXEL_BYTES`], so a byte count is a shift away from a texel
/// count (the shader does the same, since a downlevel target has no
/// `firstTrailingBit`).
pub const TEXEL_BYTES_SHIFT: u32 = 4;

/// Bytes in one `Rgba32Uint` texel: four channels of four bytes.
pub const TEXEL_BYTES: u32 = 1 << TEXEL_BYTES_SHIFT;

/// 1-byte alpha values packed into one texel.
pub const ALPHAS_PER_TEXEL: u32 = TEXEL_BYTES;

/// 1-byte alpha values packed into one texel channel.
pub const ALPHAS_PER_CHANNEL: u32 = 4;

/// The height a resource texture starts at, before any content forces it to
/// grow.
pub const MIN_RESOURCE_TEXTURE_HEIGHT: u32 = 1;

/// The `bytes_per_row` of a resource texture `width` texels wide.
#[must_use]
pub const fn resource_bytes_per_row(width: u32) -> u32 {
    width << TEXEL_BYTES_SHIFT
}

/// The total byte footprint of a resource texture of `width` x `height`
/// texels — the size an upload buffer is padded to.
#[must_use]
pub const fn resource_texture_bytes(width: u32, height: u32) -> u64 {
    (width as u64 * height as u64) << TEXEL_BYTES_SHIFT
}

/// A resource-texture descriptor: `Rgba32Uint`, single mip, single sample,
/// sampled-and-uploadable.
#[must_use]
pub fn resource_texture_descriptor(
    label: &str,
    width: u32,
    height: u32,
) -> wgpu::TextureDescriptor<'_> {
    wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: RESOURCE_TEXTURE_FORMAT,
        usage: RESOURCE_TEXTURE_USAGES,
        view_formats: &[],
    }
}

/// The descriptor for an alpha texture of `width` x `height` texels.
#[must_use]
pub fn alpha_texture_descriptor(width: u32, height: u32) -> wgpu::TextureDescriptor<'static> {
    resource_texture_descriptor("frust-engine alpha texture", width, height)
}

/// The alpha-texture height needed to hold `alphas_len` coverage bytes in a
/// texture `resource_texture_dim` texels wide.
///
/// Each texel holds [`ALPHAS_PER_TEXEL`] alpha values, so one row holds
/// `resource_texture_dim << 4` of them. The result never drops below
/// [`MIN_RESOURCE_TEXTURE_HEIGHT`], since a texture of height zero cannot be
/// created.
///
/// # Errors
///
/// Returns [`EngineError::AlphaCapacity`] when the required height exceeds
/// `resource_texture_dim` — the point at which the reference renderer
/// asserts. Coverage past that ceiling has nowhere to live, and a frame path
/// reports it rather than panicking.
pub fn alpha_texture_height(
    alphas_len: usize,
    resource_texture_dim: u32,
) -> Result<u32, EngineError> {
    let alphas_len = u32::try_from(alphas_len).map_err(|_| EngineError::AlphaCapacity)?;
    let required = alphas_len.div_ceil(resource_texture_dim << TEXEL_BYTES_SHIFT);
    if required > resource_texture_dim {
        return Err(EngineError::AlphaCapacity);
    }
    Ok(required.max(MIN_RESOURCE_TEXTURE_HEIGHT))
}

/// The height an alpha texture currently `current_height` tall must be
/// recreated at to hold `alphas_len` coverage bytes, or `None` when the
/// existing texture already fits.
///
/// The texture grows only: a frame needing fewer alphas than the last one
/// keeps the taller texture rather than reallocating.
///
/// # Errors
///
/// Returns [`EngineError::AlphaCapacity`] on the same over-capacity condition
/// as [`alpha_texture_height`].
pub fn grow_alpha_texture_height(
    current_height: u32,
    alphas_len: usize,
    resource_texture_dim: u32,
) -> Result<Option<u32>, EngineError> {
    let required = alpha_texture_height(alphas_len, resource_texture_dim)?;
    Ok((required > current_height).then_some(required))
}

/// Where one alpha value lives in an alpha texture: its texel, the channel
/// within that texel, and the byte within that channel.
///
/// The shader reconstructs the same address from a strip's `col_idx` with
/// shifts and masks, using [`GpuConfig::alphas_tex_width_bits`] for the row
/// stride.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlphaAddress {
    /// Texel column.
    pub x: u32,
    /// Texel row.
    pub y: u32,
    /// Channel within the texel (0-3).
    pub channel: u32,
    /// Byte within the channel (0-3).
    pub byte: u32,
}

impl AlphaAddress {
    /// The offset of this address in an upload buffer of a texture `width`
    /// texels wide — the inverse of [`alpha_address`], which is what makes
    /// the packing round-trippable.
    #[must_use]
    pub const fn byte_offset(self, width: u32) -> u32 {
        ((self.y * width + self.x) << TEXEL_BYTES_SHIFT)
            + self.channel * ALPHAS_PER_CHANNEL
            + self.byte
    }
}

/// The address of alpha value `alpha_idx` in an alpha texture `width` texels
/// wide.
///
/// Alpha values are laid out linearly: 16 per texel, texels left to right
/// then top to bottom, so a value's byte offset equals its index and a
/// texel-boundary or row-boundary crossing needs no padding.
#[must_use]
pub const fn alpha_address(alpha_idx: u32, width: u32) -> AlphaAddress {
    let texel = alpha_idx >> TEXEL_BYTES_SHIFT;
    let within = alpha_idx & (ALPHAS_PER_TEXEL - 1);
    AlphaAddress {
        x: texel % width,
        y: texel / width,
        channel: within / ALPHAS_PER_CHANNEL,
        byte: within % ALPHAS_PER_CHANNEL,
    }
}

/// Runs `upload` over `alphas` padded with zeroes to the full byte footprint
/// of a `width` x `height` alpha texture, then truncates it back to its
/// original length.
///
/// A queue write covers the whole texture extent, so the source slice must be
/// the full footprint even when the frame produced fewer alphas. Padding in
/// place and truncating afterwards keeps the caller's buffer (and its grown
/// capacity) reusable across frames instead of allocating a staging copy per
/// frame.
pub fn with_padded_alphas<R>(
    alphas: &mut Vec<u8>,
    width: u32,
    height: u32,
    upload: impl FnOnce(&[u8]) -> R,
) -> R {
    let original_len = alphas.len();
    let padded_len = resource_texture_bytes(width, height);
    let padded_len = usize::try_from(padded_len)
        .unwrap_or(usize::MAX)
        .max(original_len);
    alphas.resize(padded_len, 0);
    let result = upload(alphas);
    alphas.truncate(original_len);
    result
}
