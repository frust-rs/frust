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
    /// The strip's own alpha index becomes the instance's first alpha column,
    /// and the instance is dense across its whole width: every column samples
    /// coverage.
    #[must_use]
    pub fn from_strip(strip: &Strip, width: u16, draw: StripDraw) -> Self {
        Self {
            x: strip.x,
            y: strip.y,
            width,
            dense_width_or_rect_height: width,
            col_idx_or_rect_frac: strip.alpha_idx(),
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
