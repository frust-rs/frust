//! The strip instance the vertex shader steps over, and the conversion into
//! it from a rasterized [`Strip`].
//!
//! One [`GpuStrip`] is one instance of a four-vertex quad: there is no
//! geometry buffer at all, the vertex shader builds the quad's corners from
//! the instance's own fields (see [`GpuStrip::vertex_range`]).

use bytemuck::{Pod, Zeroable};
use core::ops::Range;
use frust_gpu::VertexLayout;
use vello_common::strip::Strip;
use vello_common::tile::Tile;

const _: () = assert!(
    Tile::HEIGHT == 4,
    "the strip shaders require `Tile::HEIGHT` to be 4",
);

/// Bit 31 of [`GpuStrip::paint_and_rect_flag`], marking an instance as a
/// whole rectangle rather than a strip: the shader then reads
/// [`GpuStrip::dense_width_or_rect_height`] as a height and
/// [`GpuStrip::col_idx_or_rect_frac`] as a coverage fraction.
pub const RECT_STRIP_FLAG: u32 = 1 << 31;

/// Bit the colour source starts at in [`GpuStrip::paint_and_rect_flag`].
const COLOR_SOURCE_SHIFT: u32 = 29;

/// Bit the paint type starts at in [`GpuStrip::paint_and_rect_flag`].
const PAINT_TYPE_SHIFT: u32 = 26;

/// Mask covering the encoded-paint texel index in
/// [`GpuStrip::paint_and_rect_flag`] — everything below the paint type.
pub const PAINT_TEXTURE_INDEX_MASK: u32 = (1 << PAINT_TYPE_SHIFT) - 1;

/// The colour source saying the fragment shader reads
/// [`GpuStrip::payload`] rather than sampling a rendered layer.
///
/// The only source the engine emits: layer compositing is later work, and an
/// instance that named the layer source would sample the placeholder view
/// bound in its place.
const COLOR_SOURCE_PAYLOAD: u32 = 0;

/// How the fragment shader turns an instance's payload into colour.
///
/// The discriminants are the shader's own paint-type numbering, not an
/// arbitrary ordering — they are written into
/// [`GpuStrip::paint_and_rect_flag`] as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum PaintType {
    /// The payload is a premultiplied RGBA8 colour, used directly.
    Solid = 0,
    /// The payload is a sample position; the record names an atlas image.
    Image = 1,
    /// The payload is a sample position; the record names a linear gradient.
    LinearGradient = 2,
    /// The payload is a sample position; the record names a radial gradient.
    RadialGradient = 3,
    /// The payload is a sample position; the record names a sweep gradient.
    SweepGradient = 4,
    /// The payload is a sample position; the record names a blurred rounded
    /// rectangle.
    BlurredRoundedRect = 5,
}

/// The packed paint descriptor for a `paint_type` instance whose encoded-paint
/// record starts at `texel_index`.
///
/// [`PaintType::Solid`] indexes no record and passes `0`; every other type
/// carries the texel its record starts at, which is what
/// `load_encoded_paint_texel` addresses the paint texture by. The index is
/// masked rather than reported, because it is produced by this crate's own
/// serialization — a paint table large enough to overflow 26 bits is refused
/// by the paint texture's own capacity check long before it reaches here.
#[must_use]
pub const fn pack_paint_descriptor(paint_type: PaintType, texel_index: u32) -> u32 {
    (COLOR_SOURCE_PAYLOAD << COLOR_SOURCE_SHIFT)
        | ((paint_type as u32) << PAINT_TYPE_SHIFT)
        | (texel_index & PAINT_TEXTURE_INDEX_MASK)
}

/// The alpha *column* the strip shader addresses coverage by, from the byte
/// index a [`Strip`] carries.
///
/// The two are different units, and the conversion is the shader's own. A
/// strip's `alpha_idx` counts coverage *bytes* from the start of the frame's
/// buffer; [`GpuStrip::col_idx_or_rect_frac`] is read as a column ordinal,
/// which the fragment stage turns into a texel with `col / 4` and a channel
/// within it with `col % 4` — one channel holding one pixel column's
/// `Tile::HEIGHT` coverage bytes. So a column is `Tile::HEIGHT` bytes wide,
/// which is the same unit [`Strip::width_to`] already reports a strip's width
/// in.
///
/// Getting this wrong is invisible on fully-covered geometry and total on
/// anti-aliased geometry: every alpha-sampled span reads past its own
/// coverage, lands on unwritten texels, and resolves to zero alpha.
#[must_use]
pub const fn alpha_column(alpha_idx: u32) -> u32 {
    alpha_idx / Tile::HEIGHT as u32
}

/// One strip instance, matching the shaders' `StripInstance` byte for byte.
///
/// Three of the fields are overloaded by [`RECT_STRIP_FLAG`], which is why
/// they are named for both readings.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuStrip {
    /// Left edge in pixels.
    pub x: u16,
    /// Top edge in pixels (a multiple of `Tile::HEIGHT`).
    pub y: u16,
    /// Width in pixels.
    pub width: u16,
    /// Width of the alpha-sampled (dense) part in pixels, or, for a rect
    /// instance, the rect's height in pixels.
    pub dense_width_or_rect_height: u16,
    /// Index of the instance's first alpha column in the alpha texture, or,
    /// for a rect instance, its packed coverage fraction.
    pub col_idx_or_rect_frac: u32,
    /// Paint-dependent payload — a packed color, or the coordinates the paint
    /// is sampled at.
    pub payload: u32,
    /// Packed paint descriptor, with [`RECT_STRIP_FLAG`] in bit 31.
    pub paint_and_rect_flag: u32,
    /// Painter's-order index driving early-z rejection: the backmost draw is
    /// 0 and each draw in front of it increments.
    pub depth_index: u32,
}

const _: () = assert!(
    size_of::<GpuStrip>() == 24,
    "`GpuStrip` must stay 24 bytes — the shaders' `StripInstance` layout",
);
const _: () = assert!(
    size_of::<GpuStrip>() == size_of::<u16>() * 4 + size_of::<u32>() * 4,
    "`GpuStrip` must stay padding-free — six `Uint32` vertex attributes read it",
);

/// The per-draw values every strip instance of one draw shares.
///
/// Carried separately from the geometry because a draw resolves them once
/// (its paint, its painter's-order index) and every strip it emits repeats
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripDraw {
    /// [`GpuStrip::payload`].
    pub payload: u32,
    /// The packed paint descriptor, without [`RECT_STRIP_FLAG`].
    pub paint: u32,
    /// [`GpuStrip::depth_index`].
    pub depth_index: u32,
}

impl GpuStrip {
    /// Vertices in the quad each instance expands to.
    pub const QUAD_VERTICES: u32 = 4;

    /// The six `Uint32` vertex attributes the 24-byte instance is read
    /// through, at shader locations 0-5.
    ///
    /// The `u16` pairs are read as packed `u32`s and unpacked in the shader,
    /// so the attribute list stays uniform.
    #[must_use]
    pub fn vertex_attributes() -> [wgpu::VertexAttribute; 6] {
        wgpu::vertex_attr_array![
            0 => Uint32,
            1 => Uint32,
            2 => Uint32,
            3 => Uint32,
            4 => Uint32,
            5 => Uint32,
        ]
    }

    /// The instance-stepped vertex buffer layout over [`GpuStrip`].
    #[must_use]
    pub fn vertex_layout() -> VertexLayout {
        VertexLayout {
            array_stride: size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: Self::vertex_attributes().to_vec(),
        }
    }

    /// The vertex range of a strip draw: one quad, built in the shader from
    /// the vertex index, with no vertex buffer behind it.
    #[must_use]
    pub fn vertex_range() -> Range<u32> {
        0..Self::QUAD_VERTICES
    }

    /// The instance range of a strip draw covering `count` instances from
    /// `first_instance` — paired with [`Self::vertex_range`] for one
    /// `draw` call.
    #[must_use]
    pub fn instance_range(first_instance: u32, count: u32) -> Range<u32> {
        first_instance..first_instance.saturating_add(count)
    }

    /// The alpha-sampled instance for `strip`, `width` pixels wide.
    ///
    /// The strip's alpha index becomes the instance's first alpha *column*,
    /// and the instance is dense across its whole width: every column samples
    /// coverage.
    #[must_use]
    pub fn from_strip(strip: &Strip, width: u16, draw: StripDraw) -> Self {
        Self {
            x: strip.x,
            y: strip.y,
            width,
            dense_width_or_rect_height: width,
            col_idx_or_rect_frac: alpha_column(strip.alpha_idx()),
            payload: draw.payload,
            paint_and_rect_flag: draw.paint,
            depth_index: draw.depth_index,
        }
    }

    /// The alpha-sampled instance for `strip`, taking its width from `next` —
    /// the strip immediately after it in the same run, which is where a
    /// strip's extent is recorded.
    #[must_use]
    pub fn from_strip_pair(strip: &Strip, next: &Strip, draw: StripDraw) -> Self {
        Self::from_strip(strip, strip.width_to(next), draw)
    }

    /// The solid instance filling the gap between `strip` and `next`, or
    /// `None` when there is no gap to fill.
    ///
    /// A gap is filled only when `next` carries the fill-gap flag and sits on
    /// the same strip row: the flag means "the winding between me and the
    /// previous strip is non-zero", which says nothing about a strip on
    /// another row. The instance samples no coverage, so its dense width and
    /// column index are both zero.
    #[must_use]
    pub fn gap_fill(strip: &Strip, next: &Strip, draw: StripDraw) -> Option<Self> {
        if !next.fill_gap() || next.y != strip.y {
            return None;
        }
        let gap_x = strip.x.saturating_add(strip.width_to(next));
        let gap_width = next.x.saturating_sub(gap_x);
        if gap_width == 0 {
            return None;
        }
        Some(Self::solid_fill(gap_x, strip.y, gap_width, draw))
    }

    /// A solid instance covering one strip row, sampling no coverage.
    #[must_use]
    pub fn solid_fill(x: u16, y: u16, width: u16, draw: StripDraw) -> Self {
        Self {
            x,
            y,
            width,
            dense_width_or_rect_height: 0,
            col_idx_or_rect_frac: 0,
            payload: draw.payload,
            paint_and_rect_flag: draw.paint,
            depth_index: draw.depth_index,
        }
    }

    /// A rectangle instance of `width` x `height` pixels with packed coverage
    /// fraction `frac`, flagged with [`RECT_STRIP_FLAG`].
    ///
    /// Unlike a strip, a rect instance is not bound to one strip row: its
    /// height is carried in the instance itself.
    #[must_use]
    pub fn from_rect(x: u16, y: u16, width: u16, height: u16, frac: u32, draw: StripDraw) -> Self {
        Self {
            x,
            y,
            width,
            dense_width_or_rect_height: height,
            col_idx_or_rect_frac: frac,
            payload: draw.payload,
            paint_and_rect_flag: draw.paint | RECT_STRIP_FLAG,
            depth_index: draw.depth_index,
        }
    }

    /// Whether this instance is a rectangle rather than a strip.
    #[must_use]
    pub const fn is_rect(&self) -> bool {
        self.paint_and_rect_flag & RECT_STRIP_FLAG != 0
    }

    /// The packed paint descriptor with [`RECT_STRIP_FLAG`] masked off.
    #[must_use]
    pub const fn paint(&self) -> u32 {
        self.paint_and_rect_flag & !RECT_STRIP_FLAG
    }
}
