//! The `Config` uniform every strip shader binds.
//!
//! Named [`GpuConfig`] rather than `Config` because the crate root already
//! owns a `config` module of process-wide kill switches ([`crate::config`]);
//! this one is the GPU-side uniform buffer's layout and nothing else.
//!
//! Field order and size are part of the shader contract — see the module
//! header of [`super`].

use bytemuck::{Pod, Zeroable};
use vello_common::tile::Tile;

/// The uniform block the strip shaders read once per draw.
///
/// Laid out to match the reference renderer's `Config` byte for byte: eight
/// 4-byte scalars in a 16-byte-aligned block.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuConfig {
    /// Width of the render target in pixels.
    pub width: u32,
    /// Height of the render target in pixels.
    pub height: u32,
    /// Height of one strip in pixels — always `Tile::HEIGHT`.
    pub strip_height: u32,
    /// `log2` of the alpha texture's width in texels.
    ///
    /// Pre-computed on the CPU because a downlevel (GLES 3.0 / WebGL2)
    /// target has no `firstTrailingBit`; the shader shifts by this instead of
    /// dividing.
    pub alphas_tex_width_bits: u32,
    /// `log2` of the encoded-paint texture's width in texels, pre-computed
    /// for the same reason as [`Self::alphas_tex_width_bits`].
    pub encoded_paints_tex_width_bits: u32,
    /// Horizontal offset applied to every strip, in pixels.
    pub strip_offset_x: i32,
    /// Vertical offset applied to every strip, in pixels.
    pub strip_offset_y: i32,
    /// Whether the shader negates the y component of its NDC position
    /// (non-zero) or leaves it alone (zero).
    ///
    /// Kept for a future GLSL/WebGL backend: transpiling WGSL to GLSL applies
    /// a y-flip to reconcile WebGPU's y-down clip space with a WebGL
    /// framebuffer's y-up one. Rendering straight into a caller-provided
    /// framebuffer wants that flip undone rather than a second framebuffer
    /// allocated and blitted, and the rest of the strip pipeline (slot
    /// textures included) assumes y-down throughout. Every backend this crate
    /// drives today leaves it zero.
    pub negate_ndc: u32,
}

const _: () = assert!(
    size_of::<GpuConfig>() == 32,
    "`GpuConfig` must stay 32 bytes — the shaders' `Config` uniform layout",
);
const _: () = assert!(
    align_of::<GpuConfig>() == 16,
    "`GpuConfig` must stay 16-byte aligned — uniform-buffer layout rules",
);

impl GpuConfig {
    /// The uniform's size in bytes, for a buffer binding's `min_binding_size`.
    pub const SIZE: u64 = size_of::<Self>() as u64;

    /// A config for a `width` x `height` target sampling resource textures
    /// `alphas_tex_width` and `encoded_paints_tex_width` texels wide, with no
    /// strip offset and no NDC negation.
    ///
    /// Both texture widths must be powers of two: the shader reconstructs
    /// them as `1 << bits` (see [`tex_width_bits`]).
    #[must_use]
    pub fn new(
        width: u32,
        height: u32,
        alphas_tex_width: u32,
        encoded_paints_tex_width: u32,
    ) -> Self {
        Self {
            width,
            height,
            strip_height: u32::from(Tile::HEIGHT),
            alphas_tex_width_bits: tex_width_bits(alphas_tex_width),
            encoded_paints_tex_width_bits: tex_width_bits(encoded_paints_tex_width),
            strip_offset_x: 0,
            strip_offset_y: 0,
            negate_ndc: 0,
        }
    }

    /// The same config with strips offset by `(x, y)` pixels.
    #[must_use]
    pub const fn with_strip_offset(mut self, x: i32, y: i32) -> Self {
        self.strip_offset_x = x;
        self.strip_offset_y = y;
        self
    }

    /// The same config with NDC y negation switched on or off
    /// ([`Self::negate_ndc`]).
    #[must_use]
    pub const fn with_negate_ndc(mut self, negate: bool) -> Self {
        self.negate_ndc = negate as u32;
        self
    }
}

/// `log2` of a resource texture's width in texels.
///
/// `width` must be a power of two — every resource texture is sized from
/// `frust_gpu::TierCaps::resource_texture_dim`, which is. A width that is not
/// makes the shader's `1 << bits` reconstruction disagree with the real
/// texture, so the precondition is checked in debug builds; release builds
/// take the trailing-zero count as-is rather than panicking on a frame path.
#[must_use]
pub fn tex_width_bits(width: u32) -> u32 {
    debug_assert!(
        width.is_power_of_two(),
        "resource texture width must be a power of two, got {width}",
    );
    width.trailing_zeros()
}
